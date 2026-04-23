use std::path::PathBuf;

use crate::encoding;
use crate::error::WkmError;
use crate::git::{GitBranches, GitDiscovery, GitWorktrees};
use crate::repo::RepoContext;
use crate::state;
use crate::state::lock::WkmLock;
use crate::state::types::{BranchEntry, WorkspaceEntry, WorktreeBackend};

/// Options for creating a worktree.
pub struct CreateOptions {
    /// Branch name to create.
    pub branch: String,
    /// Base branch to branch from (defaults to current branch).
    pub base: Option<String>,
    /// Description for the branch.
    pub description: Option<String>,
    /// Optional workspace alias to attach to the new worktree.
    pub name: Option<String>,
}

/// Result of worktree creation.
pub struct CreateResult {
    pub branch: String,
    pub worktree_path: PathBuf,
    pub created_branch: bool,
}

/// Create a new worktree for a branch.
pub fn create(
    ctx: &RepoContext,
    git: &(impl GitDiscovery + GitBranches + GitWorktrees),
    opts: &CreateOptions,
) -> Result<CreateResult, WkmError> {
    // Validate the alias up front so we fail before touching git/disk.
    if let Some(ref alias) = opts.name {
        encoding::validate_workspace_alias(alias).map_err(WkmError::InvalidWorkspaceAlias)?;
    }

    let lock = WkmLock::acquire(&ctx.lock_path)?;

    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    // Check for in-progress operation
    if wkm_state.wal.is_some() {
        return Err(WkmError::OperationInProgress);
    }

    // Reject duplicate aliases before creating anything.
    if let Some(ref alias) = opts.name
        && let Some(existing) = wkm_state.workspaces.get(alias)
    {
        return Err(WkmError::WorkspaceAliasExists(
            alias.clone(),
            existing.worktree_path.clone(),
        ));
    }

    // Determine the parent branch
    let parent = opts
        .base
        .clone()
        .or_else(|| git.current_branch(&ctx.main_worktree).ok().flatten())
        .unwrap_or_else(|| wkm_state.config.base_branch.clone());

    // Generate opaque directory name (bounded retry on collision)
    let dir_name = (0..100)
        .find_map(|_| {
            let id = encoding::generate_worktree_id();
            let candidate = ctx.storage_dir.join(&id).join(&ctx.repo_name);
            if !candidate.exists() { Some(id) } else { None }
        })
        .ok_or_else(|| {
            WkmError::Other("worktree ID collision: exhausted 100 attempts".to_string())
        })?;

    let worktree_path = ctx.storage_dir.join(&dir_name).join(&ctx.repo_name);

    let mut created_branch = false;

    if git.branch_exists(&opts.branch)? {
        // Check if already checked out somewhere
        let worktrees = git.worktree_list()?;
        for wt in &worktrees {
            if wt.branch.as_deref() == Some(&opts.branch) {
                return Err(WkmError::BranchCheckedOut(
                    opts.branch.clone(),
                    wt.path.clone(),
                ));
            }
        }
    } else {
        // Create the branch
        git.create_branch(&opts.branch, &parent)?;
        created_branch = true;
    }

    // Create the worktree
    std::fs::create_dir_all(&ctx.storage_dir)?;

    let jj_workspace_name = match wkm_state.config.worktree_backend {
        WorktreeBackend::Git | WorktreeBackend::GitJj => {
            // Both Git and GitJj start with a proper git worktree
            git.worktree_add(&worktree_path, &opts.branch)?;

            if wkm_state.config.worktree_backend == WorktreeBackend::GitJj {
                // Dual registration: create jj workspace at temp, move .jj/ into git worktree
                let ws_name = sanitize_jj_workspace_name(&opts.branch);
                setup_jj_workspace(ctx, &worktree_path, &ws_name, &opts.branch)?;
                Some(ws_name)
            } else {
                None
            }
        }
        WorktreeBackend::Jj => {
            // jj-only: create workspace directly (no git worktree)
            let ws_name = sanitize_jj_workspace_name(&opts.branch);
            let jj = crate::git::jj_cli::JjCli::new(&ctx.main_worktree);
            jj.workspace_add(&worktree_path, &ws_name, &opts.branch)?;
            Some(ws_name)
        }
    };

    // Update state
    let now = chrono::Utc::now().to_rfc3339();
    wkm_state.branches.insert(
        opts.branch.clone(),
        BranchEntry {
            parent: Some(parent),
            worktree_path: Some(worktree_path.clone()),
            stash_commit: None,
            jj_workspace_name,
            description: opts.description.clone(),
            created_at: now.clone(),
            previous_branch: None,
        },
    );
    if let Some(ref alias) = opts.name {
        wkm_state.workspaces.insert(
            alias.clone(),
            WorkspaceEntry {
                worktree_path: worktree_path.clone(),
                created_at: now,
                description: None,
            },
        );
    }
    state::write_state(&ctx.state_path, &wkm_state)?;

    drop(lock);

    Ok(CreateResult {
        branch: opts.branch.clone(),
        worktree_path,
        created_branch,
    })
}

/// Options for removing a worktree.
#[derive(Debug, Default, Clone)]
pub struct RemoveOptions<'a> {
    /// Branch name (defaults to current branch).
    pub branch: Option<&'a str>,
    /// Force worktree removal even if dirty.
    pub force: bool,
    /// Drop any pending auto-stash on the branch instead of erroring.
    pub drop_stash: bool,
}

/// Remove a worktree for a branch, and drop the wkm state entry.
///
/// The underlying git branch is preserved; only the worktree directory and
/// the wkm-tracked metadata are removed. Errors with `PendingStash` if the
/// branch has a recorded auto-stash, unless `opts.drop_stash` is set.
pub fn remove(
    ctx: &RepoContext,
    git: &(impl GitDiscovery + GitBranches + GitWorktrees),
    opts: &RemoveOptions<'_>,
) -> Result<String, WkmError> {
    let lock = WkmLock::acquire(&ctx.lock_path)?;

    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    if wkm_state.wal.is_some() {
        return Err(WkmError::OperationInProgress);
    }

    // Determine which branch to remove
    let branch_name = if let Some(b) = opts.branch {
        b.to_string()
    } else {
        // Default to current branch
        let cwd = std::env::current_dir()?;
        git.current_branch(&cwd)?
            .ok_or_else(|| WkmError::Other("detached HEAD".to_string()))?
    };

    let entry = wkm_state
        .branches
        .get(&branch_name)
        .ok_or_else(|| WkmError::BranchNotTracked(branch_name.clone()))?;

    let worktree_path = entry
        .worktree_path
        .clone()
        .ok_or_else(|| WkmError::NoWorktree(branch_name.clone()))?;

    // Guard against silently dropping an auto-stash. `drop_stash` opts out.
    if entry.stash_commit.is_some() && !opts.drop_stash {
        return Err(WkmError::PendingStash(branch_name));
    }

    // Check if we're inside the worktree
    if let Ok(cwd) = std::env::current_dir()
        && cwd.starts_with(&worktree_path)
    {
        return Err(WkmError::RemoveFromInside);
    }

    // Forget jj workspace if this was a dual or jj-only worktree
    let jj_ws_name = entry.jj_workspace_name.clone();
    if let Some(ref ws_name) = jj_ws_name {
        let jj = crate::git::jj_cli::JjCli::new(&ctx.main_worktree);
        let _ = jj.workspace_forget(ws_name);
    }

    // Drop the wkm state entry entirely — the git branch itself is preserved.
    wkm_state.branches.remove(&branch_name);
    // Drop any workspace alias pointing at this directory.
    let worktree_path_ref = worktree_path.clone();
    wkm_state
        .workspaces
        .retain(|_, v| v.worktree_path != worktree_path_ref);
    state::write_state(&ctx.state_path, &wkm_state)?;

    // Try to rename the worktree directory for background deletion.
    // The ".wkm-removing" suffix signals that this directory is pending cleanup.
    let removing_path = worktree_path.with_extension("wkm-removing");
    let renamed = if worktree_path.exists() {
        std::fs::rename(&worktree_path, &removing_path).is_ok()
    } else {
        false
    };

    // Prune git worktree metadata (the original path no longer exists after rename)
    let _ = git.worktree_prune();

    // Clean up any _wkm/ hold branches for this branch
    let hold_branch = format!("_wkm/hold/{branch_name}");
    if git.branch_exists(&hold_branch)? {
        let _ = git.delete_branch(&hold_branch, true);
    }

    // Delete the directory: background if we renamed, synchronous fallback otherwise
    if renamed {
        spawn_background_delete(&removing_path);
    } else if worktree_path.exists() {
        // Fallback: rename failed (cross-filesystem), use synchronous removal
        git.worktree_remove(&worktree_path, opts.force)?;
    }

    drop(lock);

    Ok(branch_name)
}

/// Spawn a detached `rm -rf` process to delete a directory in the background.
fn spawn_background_delete(path: &std::path::Path) {
    let path_str = match path.to_str() {
        Some(s) => s.to_string(),
        None => return, // non-UTF8 path, skip background delete
    };

    let _ = std::process::Command::new("rm")
        .args(["-rf", &path_str])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Sanitize a branch name into a valid jj workspace ID.
/// Replaces `/` and other problematic characters with `-`.
fn sanitize_jj_workspace_name(branch: &str) -> String {
    branch
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == ' ' {
                '-'
            } else {
                c
            }
        })
        .collect()
}

/// Set up a jj workspace in an existing git worktree (dual registration).
///
/// 1. Create jj workspace at a temp sibling path
/// 2. Move `.jj/` from temp into the git worktree
/// 3. Write `.jj/.gitignore` (jj omits this in secondary workspaces)
/// 4. Remove the temp directory
fn setup_jj_workspace(
    ctx: &RepoContext,
    worktree_path: &std::path::Path,
    ws_name: &str,
    branch: &str,
) -> Result<(), WkmError> {
    let jj = crate::git::jj_cli::JjCli::new(&ctx.main_worktree);

    // Create temp directory as a sibling of the worktree so relative paths match
    let tmp_name = format!(".wkm-jj-tmp-{}", encoding::generate_worktree_id());
    let tmp_path = worktree_path
        .parent()
        .unwrap_or(worktree_path)
        .join(&tmp_name);

    // Create jj workspace at temp location pointed at the branch
    if let Err(e) = jj.workspace_add(&tmp_path, ws_name, branch) {
        // Clean up temp dir on failure
        let _ = std::fs::remove_dir_all(&tmp_path);
        return Err(e);
    }

    // Move .jj/ from temp to the git worktree
    let tmp_jj = tmp_path.join(".jj");
    let dest_jj = worktree_path.join(".jj");
    if let Err(e) = std::fs::rename(&tmp_jj, &dest_jj) {
        // Clean up: forget the workspace and remove temp
        let _ = jj.workspace_forget(ws_name);
        let _ = std::fs::remove_dir_all(&tmp_path);
        return Err(WkmError::Other(format!(
            "failed to move .jj/ to worktree: {e}"
        )));
    }

    // Write .jj/.gitignore — jj creates this in main repos but not secondary workspaces
    let gitignore_path = dest_jj.join(".gitignore");
    if !gitignore_path.exists() {
        let _ = std::fs::write(&gitignore_path, "/*\n");
    }

    // Remove the (now empty except for checked-out files) temp directory
    let _ = std::fs::remove_dir_all(&tmp_path);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::cli::CliGit;
    use crate::ops::init::{self, InitOptions};
    use wkm_sandbox::TestRepo;

    fn setup() -> (TestRepo, RepoContext, CliGit) {
        let repo = TestRepo::new();
        let ctx = RepoContext::from_path(repo.path()).unwrap();
        let git = CliGit::new(repo.path());
        init::init(&ctx, &InitOptions::default()).unwrap();
        (repo, ctx, git)
    }

    #[test]
    fn worktree_create_new_branch() {
        let (_repo, ctx, git) = setup();

        let result = create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feature".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();

        assert_eq!(result.branch, "feature");
        assert!(result.created_branch);
        assert!(result.worktree_path.exists());
        assert!(git.branch_exists("feature").unwrap());

        // State should be updated
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        let entry = &wkm_state.branches["feature"];
        assert_eq!(entry.parent, Some("main".to_string()));
        assert!(entry.worktree_path.is_some());
    }

    #[test]
    fn worktree_create_existing_branch() {
        let (repo, ctx, git) = setup();
        repo.create_branch("existing");

        let result = create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "existing".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();

        assert!(!result.created_branch);
        assert!(result.worktree_path.exists());
    }

    #[test]
    fn worktree_create_already_checked_out() {
        let (_repo, ctx, git) = setup();

        // Create first worktree
        create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feature".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();

        // Try to create another for the same branch
        let result = create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feature".to_string(),
                base: None,
                description: None,
                name: None,
            },
        );

        assert!(matches!(result, Err(WkmError::BranchCheckedOut(_, _))));
    }

    #[test]
    fn worktree_create_with_base() {
        let (repo, ctx, git) = setup();
        repo.create_branch("develop");

        let result = create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feature".to_string(),
                base: Some("develop".to_string()),
                description: None,
                name: None,
            },
        )
        .unwrap();

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert_eq!(
            wkm_state.branches["feature"].parent,
            Some("develop".to_string())
        );
        assert!(result.created_branch);
    }

    #[test]
    fn worktree_remove_basic() {
        let (_repo, ctx, git) = setup();

        create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feature".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        let wt_path = wkm_state.branches["feature"]
            .worktree_path
            .as_ref()
            .unwrap()
            .clone();
        assert!(wt_path.exists());

        let removed = remove(
            &ctx,
            &git,
            &RemoveOptions {
                branch: Some("feature"),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(removed, "feature");

        // Original worktree path should be gone (renamed to .wkm-removing)
        assert!(!wt_path.exists());

        // Branch entry should be gone from wkm state entirely
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(!wkm_state.branches.contains_key("feature"));

        // Git branch should still exist
        assert!(git.branch_exists("feature").unwrap());

        // Git should no longer track the worktree
        let worktrees = git.worktree_list().unwrap();
        assert!(!worktrees.iter().any(|w| w.path == wt_path));
    }

    #[test]
    fn worktree_remove_renames_to_wkm_removing() {
        let (_repo, ctx, git) = setup();

        create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feature".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        let wt_path = wkm_state.branches["feature"]
            .worktree_path
            .as_ref()
            .unwrap()
            .clone();
        let removing_path = wt_path.with_extension("wkm-removing");

        remove(
            &ctx,
            &git,
            &RemoveOptions {
                branch: Some("feature"),
                ..Default::default()
            },
        )
        .unwrap();

        // Original path gone
        assert!(!wt_path.exists());

        // .wkm-removing may still exist briefly (background rm -rf)
        // but the state entry should already be gone
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(!wkm_state.branches.contains_key("feature"));

        // Clean up for test
        let _ = std::fs::remove_dir_all(&removing_path);
    }

    #[test]
    fn worktree_remove_errors_on_pending_stash() {
        let (_repo, ctx, git) = setup();

        create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feature".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();

        // Simulate a pending auto-stash recorded on the entry
        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.get_mut("feature").unwrap().stash_commit =
            Some("deadbeefcafebabe".to_string());
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let result = remove(
            &ctx,
            &git,
            &RemoveOptions {
                branch: Some("feature"),
                ..Default::default()
            },
        );
        assert!(
            matches!(result, Err(WkmError::PendingStash(ref b)) if b == "feature"),
            "expected PendingStash error, got: {result:?}"
        );

        // State must be unchanged
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(wkm_state.branches.contains_key("feature"));
        assert_eq!(
            wkm_state.branches["feature"].stash_commit.as_deref(),
            Some("deadbeefcafebabe"),
        );
    }

    #[test]
    fn worktree_remove_with_drop_stash_succeeds() {
        let (_repo, ctx, git) = setup();

        create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feature".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.get_mut("feature").unwrap().stash_commit =
            Some("deadbeefcafebabe".to_string());
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let removed = remove(
            &ctx,
            &git,
            &RemoveOptions {
                branch: Some("feature"),
                drop_stash: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(removed, "feature");

        // Entry should be gone
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(!wkm_state.branches.contains_key("feature"));

        // Git branch itself remains
        assert!(git.branch_exists("feature").unwrap());
    }

    #[test]
    fn worktree_create_unique_dirs() {
        let (_repo, ctx, git) = setup();

        let r1 = create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feature-a".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();

        let r2 = create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feature-b".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();

        // Different branches get different directory IDs
        assert_ne!(r1.worktree_path, r2.worktree_path);
    }

    #[test]
    fn worktree_dir_is_8_hex() {
        let (_repo, ctx, git) = setup();

        let result = create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feature".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();

        // The worktree path should be <storage_dir>/<8-hex>/<repo_name>
        let parent = result.worktree_path.parent().unwrap();
        let dir_name = parent.file_name().unwrap().to_str().unwrap();
        assert_eq!(dir_name.len(), 8);
        assert!(
            dir_name
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }
}
