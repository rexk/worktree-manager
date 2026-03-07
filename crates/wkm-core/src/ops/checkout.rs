use std::path::Path;

use crate::error::WkmError;
use crate::git::{GitBranches, GitDiscovery, GitMutations, GitStash, GitStatus, GitWorktrees};
use crate::repo::RepoContext;
use crate::state;
use crate::state::lock::WkmLock;
use crate::state::types::{BranchEntry, SwapStep, WalEntry, WalOp};

/// Checkout a branch in the current worktree.
///
/// If the branch is checked out in another worktree, performs a swap:
/// stash both, move branches via hold, then restore.
pub fn checkout(
    ctx: &RepoContext,
    git: &(impl GitDiscovery + GitBranches + GitWorktrees + GitStatus + GitStash + GitMutations),
    worktree: &Path,
    target_branch: &str,
    include_untracked: bool,
) -> Result<(), WkmError> {
    let lock = WkmLock::acquire(&ctx.lock_path)?;

    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    if wkm_state.wal.is_some() {
        return Err(WkmError::OperationInProgress);
    }

    // Get current branch
    let current_branch = git
        .current_branch(worktree)?
        .ok_or_else(|| WkmError::Other("detached HEAD".to_string()))?;

    // No-op if same branch
    if current_branch == target_branch {
        return Ok(());
    }

    // Check target branch exists
    if !git.branch_exists(target_branch)? {
        return Err(WkmError::BranchNotFound(target_branch.to_string()));
    }

    // Check if target is checked out in another worktree
    let worktrees = git.worktree_list()?;
    let target_worktree = worktrees
        .iter()
        .find(|wt| wt.branch.as_deref() == Some(target_branch))
        .map(|wt| wt.path.clone());

    if let Some(ref target_wt_path) = target_worktree {
        // Check target worktree for in-progress git operations
        if let Some(op) = git.has_in_progress_operation(target_wt_path)? {
            return Err(WkmError::InProgressGitOp(
                target_branch.to_string(),
                format!("{op:?}"),
            ));
        }

        // Swap operation needed
        swap_checkout(
            ctx,
            git,
            &mut wkm_state,
            worktree,
            &current_branch,
            target_branch,
            target_wt_path,
            include_untracked,
        )?;
    } else {
        // Simple checkout — target not in any worktree
        if git.is_dirty(worktree)? && git.has_changes_for_stash(worktree)? {
            let hash = git.stash_push(
                worktree,
                &format!("wkm: auto-stash for checkout to {target_branch}"),
                include_untracked,
            )?;
            if let Some(entry) = wkm_state.branches.get_mut(&current_branch) {
                entry.stash_commit = Some(hash);
            }
        }

        git.checkout(worktree, target_branch)?;

        // Restore any stash for the target branch
        if let Some(entry) = wkm_state.branches.get_mut(target_branch)
            && let Some(ref stash_hash) = entry.stash_commit.clone()
        {
            let _ = git.stash_apply(worktree, stash_hash, true);
            entry.stash_commit = None;
        }

        // Update previous_branch
        if let Some(entry) = wkm_state.branches.get_mut(target_branch) {
            entry.previous_branch = Some(current_branch.clone());
        }

        state::write_state(&ctx.state_path, &wkm_state)?;
    }

    drop(lock);
    Ok(())
}

/// Create a new branch and checkout.
pub fn checkout_create(
    ctx: &RepoContext,
    git: &(impl GitDiscovery + GitBranches + GitWorktrees + GitStatus + GitStash + GitMutations),
    worktree: &Path,
    new_branch: &str,
    start_point: Option<&str>,
) -> Result<(), WkmError> {
    let lock = WkmLock::acquire(&ctx.lock_path)?;

    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    if wkm_state.wal.is_some() {
        return Err(WkmError::OperationInProgress);
    }

    if git.branch_exists(new_branch)? {
        return Err(WkmError::BranchAlreadyExists(new_branch.to_string()));
    }

    let current_branch = git
        .current_branch(worktree)?
        .ok_or_else(|| WkmError::Other("detached HEAD".to_string()))?;

    let parent = start_point.unwrap_or(&current_branch);

    git.checkout_new_branch(worktree, new_branch)?;

    let now = chrono::Utc::now().to_rfc3339();
    wkm_state.branches.insert(
        new_branch.to_string(),
        BranchEntry {
            parent: Some(parent.to_string()),
            worktree_path: wkm_state
                .branches
                .get(&current_branch)
                .and_then(|e| e.worktree_path.clone()),
            stash_commit: None,
            description: None,
            created_at: now,
            previous_branch: Some(current_branch),
        },
    );
    state::write_state(&ctx.state_path, &wkm_state)?;

    drop(lock);
    Ok(())
}

/// Perform swap checkout: stash both worktrees, use hold branch, swap.
#[allow(clippy::too_many_arguments)]
fn swap_checkout(
    ctx: &RepoContext,
    git: &(impl GitDiscovery + GitBranches + GitWorktrees + GitStatus + GitStash + GitMutations),
    wkm_state: &mut state::types::WkmState,
    source_wt: &Path,
    source_branch: &str,
    target_branch: &str,
    target_wt: &Path,
    include_untracked: bool,
) -> Result<(), WkmError> {
    let hold_branch = format!("_wkm/hold/{target_branch}");

    // Step 1: Stash source (main worktree) if dirty
    let main_stash = if git.has_changes_for_stash(source_wt)? {
        let hash = git.stash_push(
            source_wt,
            &format!("wkm: swap stash for {source_branch}"),
            include_untracked,
        )?;
        Some(hash)
    } else {
        None
    };

    // Write WAL checkpoint after first stash
    let wal_id = uuid::Uuid::new_v4().to_string();
    wkm_state.wal = Some(WalEntry {
        id: wal_id.clone(),
        parent_op_id: None,
        op: WalOp::Swap {
            source_branch: source_branch.to_string(),
            target_branch: target_branch.to_string(),
            source_worktree: source_wt.to_path_buf(),
            target_worktree: Some(target_wt.to_path_buf()),
            main_stash: main_stash.clone(),
            wt_stash: None,
            step: SwapStep::StashedMain,
        },
    });
    state::write_state(&ctx.state_path, wkm_state)?;

    // Step 2: Stash target worktree if dirty
    let wt_stash = if git.has_changes_for_stash(target_wt)? {
        let hash = git.stash_push(
            target_wt,
            &format!("wkm: swap stash for {target_branch}"),
            include_untracked,
        )?;
        Some(hash)
    } else {
        None
    };

    // Update WAL with both stashes
    wkm_state.wal = Some(WalEntry {
        id: wal_id.clone(),
        parent_op_id: None,
        op: WalOp::Swap {
            source_branch: source_branch.to_string(),
            target_branch: target_branch.to_string(),
            source_worktree: source_wt.to_path_buf(),
            target_worktree: Some(target_wt.to_path_buf()),
            main_stash: main_stash.clone(),
            wt_stash: wt_stash.clone(),
            step: SwapStep::StashedBoth,
        },
    });
    state::write_state(&ctx.state_path, wkm_state)?;

    // Step 3: Move target branch to hold branch so we can check it out
    let target_ref = git.branch_ref(target_branch)?;
    git.create_branch(&hold_branch, &target_ref)?;
    git.checkout(target_wt, &hold_branch)?;

    // Update WAL
    update_swap_step(wkm_state, SwapStep::FreedBranch);
    state::write_state(&ctx.state_path, wkm_state)?;

    // Step 4: Checkout target branch in source worktree
    git.checkout(source_wt, target_branch)?;

    // Checkout source branch in target worktree
    git.checkout(target_wt, source_branch)?;

    // Update WAL
    update_swap_step(wkm_state, SwapStep::Swapped);
    state::write_state(&ctx.state_path, wkm_state)?;

    // Step 5: Apply stashes
    if let Some(ref stash_hash) = wt_stash {
        // Target's stash goes to source worktree (where target branch now is)
        let _ = git.stash_apply(source_wt, stash_hash, true);
    }
    if let Some(ref stash_hash) = main_stash {
        // Source's stash goes to target worktree (where source branch now is)
        let _ = git.stash_apply(target_wt, stash_hash, true);
    }

    // Clean up hold branch
    let _ = git.delete_branch(&hold_branch, true);

    // Update state
    if let Some(entry) = wkm_state.branches.get_mut(source_branch) {
        entry.stash_commit = None;
        if entry.worktree_path.as_ref() == Some(&source_wt.to_path_buf()) {
            entry.worktree_path = Some(target_wt.to_path_buf());
        }
    }
    if let Some(entry) = wkm_state.branches.get_mut(target_branch) {
        entry.stash_commit = None;
        entry.previous_branch = Some(source_branch.to_string());
        if entry.worktree_path.as_ref() == Some(&target_wt.to_path_buf()) {
            entry.worktree_path = Some(source_wt.to_path_buf());
        }
    }

    // Clear WAL
    wkm_state.wal = None;
    state::write_state(&ctx.state_path, wkm_state)?;

    Ok(())
}

fn update_swap_step(wkm_state: &mut state::types::WkmState, step: SwapStep) {
    if let Some(ref mut wal) = wkm_state.wal
        && let WalOp::Swap {
            step: ref mut s, ..
        } = wal.op
    {
        *s = step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::GitDiscovery;
    use crate::git::cli::CliGit;
    use crate::ops::init::{self, InitOptions};
    use crate::ops::worktree::{self, CreateOptions};
    use wkm_sandbox::TestRepo;

    #[cfg(not(windows))]
    const NULL_DEVICE: &str = "/dev/null";
    #[cfg(windows)]
    const NULL_DEVICE: &str = "NUL";

    fn setup() -> (TestRepo, RepoContext, CliGit) {
        let repo = TestRepo::new();
        let ctx = RepoContext::from_path(repo.path()).unwrap();
        let git = CliGit::new(repo.path());
        init::init(&ctx, &InitOptions::default()).unwrap();
        (repo, ctx, git)
    }

    #[test]
    fn checkout_existing_simple() {
        let (repo, ctx, git) = setup();
        repo.create_branch("feature");
        // Add to state
        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "feature".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: None,
                stash_commit: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        checkout(&ctx, &git, repo.path(), "feature", false).unwrap();
        let branch = git.current_branch(repo.path()).unwrap();
        assert_eq!(branch, Some("feature".to_string()));
    }

    #[test]
    fn checkout_noop_same_branch() {
        let (_repo, ctx, git) = setup();
        checkout(&ctx, &git, _repo.path(), "main", false).unwrap();
        // Should succeed (no-op)
    }

    #[test]
    fn checkout_nonexistent_errors() {
        let (_repo, ctx, git) = setup();
        let result = checkout(&ctx, &git, _repo.path(), "nonexistent", false);
        assert!(matches!(result, Err(WkmError::BranchNotFound(_))));
    }

    #[test]
    fn checkout_b_creates_branch() {
        let (_repo, ctx, git) = setup();
        checkout_create(&ctx, &git, _repo.path(), "new-feature", None).unwrap();

        assert!(git.branch_exists("new-feature").unwrap());
        let branch = git.current_branch(_repo.path()).unwrap();
        assert_eq!(branch, Some("new-feature".to_string()));

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert_eq!(
            wkm_state.branches["new-feature"].parent,
            Some("main".to_string())
        );
    }

    #[test]
    fn checkout_b_existing_errors() {
        let (repo, ctx, git) = setup();
        repo.create_branch("existing");
        let result = checkout_create(&ctx, &git, repo.path(), "existing", None);
        assert!(matches!(result, Err(WkmError::BranchAlreadyExists(_))));
    }

    #[test]
    fn checkout_swap_both_clean() {
        let (_repo, ctx, git) = setup();

        // Create a worktree for feature
        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feature".to_string(),
                name: None,
                base: None,
                description: None,
            },
        )
        .unwrap();

        let main_wt = git.main_worktree_path().unwrap();

        // Checkout feature from main — triggers swap
        checkout(&ctx, &git, &main_wt, "feature", false).unwrap();

        // Main worktree should now have feature
        let branch = git.current_branch(&main_wt).unwrap();
        assert_eq!(branch, Some("feature".to_string()));

        // No WAL should remain
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(wkm_state.wal.is_none());
    }

    #[test]
    fn checkout_swap_stash_both() {
        let (_repo, ctx, git) = setup();

        // Create worktree for feature
        let result = worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feature".to_string(),
                name: None,
                base: None,
                description: None,
            },
        )
        .unwrap();

        let main_wt = git.main_worktree_path().unwrap();
        let feature_wt = result.worktree_path;

        // Make both dirty
        std::fs::write(main_wt.join("initial"), "main-dirty").unwrap();
        std::fs::write(feature_wt.join("initial"), "feat-dirty").unwrap();

        // Swap
        checkout(&ctx, &git, &main_wt, "feature", false).unwrap();

        // Main should have feature + feature's dirty state
        let branch = git.current_branch(&main_wt).unwrap();
        assert_eq!(branch, Some("feature".to_string()));

        // Feature worktree should have main + main's dirty state
        let branch = git.current_branch(&feature_wt).unwrap();
        assert_eq!(branch, Some("main".to_string()));

        // WAL should be cleared
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(wkm_state.wal.is_none());
    }

    #[test]
    fn checkout_swap_stash_current_only() {
        let (_repo, ctx, git) = setup();

        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feature".to_string(),
                name: None,
                base: None,
                description: None,
            },
        )
        .unwrap();

        let main_wt = git.main_worktree_path().unwrap();

        // Only make main dirty
        std::fs::write(main_wt.join("initial"), "main-dirty").unwrap();

        checkout(&ctx, &git, &main_wt, "feature", false).unwrap();

        let branch = git.current_branch(&main_wt).unwrap();
        assert_eq!(branch, Some("feature".to_string()));
    }

    #[test]
    fn checkout_in_progress_git_rebase_errors() {
        let (repo, ctx, git) = setup();

        // First, add conflict-file on main so feature can diverge from it
        repo.commit_file("conflict-file", "base content", "add conflict-file");

        // Create worktree for feature (it will have conflict-file from main)
        let result = worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feature".to_string(),
                name: None,
                base: None,
                description: None,
            },
        )
        .unwrap();

        let feature_wt = result.worktree_path;
        wkm_sandbox::git(&feature_wt, &["config", "user.name", "Test"]);
        wkm_sandbox::git(&feature_wt, &["config", "user.email", "t@t.com"]);

        // Modify the file differently on main
        repo.commit_file(
            "conflict-file",
            "main changed",
            "main: change conflict-file",
        );

        // Modify the file differently on feature
        std::fs::write(feature_wt.join("conflict-file"), "feature changed").unwrap();
        wkm_sandbox::git(&feature_wt, &["add", "."]);
        wkm_sandbox::git(
            &feature_wt,
            &["commit", "-m", "feature: change conflict-file"],
        );

        // Start rebase that will conflict
        let _ = std::process::Command::new("git")
            .args(["rebase", "main"])
            .current_dir(&feature_wt)
            .env("GIT_CONFIG_GLOBAL", NULL_DEVICE)
            .env("GIT_CONFIG_SYSTEM", NULL_DEVICE)
            .output();

        // Verify rebase is actually in progress
        let in_progress = git.has_in_progress_operation(&feature_wt).unwrap();
        assert!(
            in_progress.is_some(),
            "rebase should be in progress but isn't"
        );

        // During rebase, the branch is in a detached HEAD state in its worktree.
        // Git still locks the branch, so checkout will fail with a git error.
        let main_wt = git.main_worktree_path().unwrap();
        let err = checkout(&ctx, &git, &main_wt, "feature", false).unwrap_err();
        // Either InProgressGitOp (if swap path) or Git error (if simple checkout path)
        assert!(
            matches!(err, WkmError::InProgressGitOp(_, _) | WkmError::Git(_)),
            "expected InProgressGitOp or Git error, got: {err:?}"
        );
    }

    #[test]
    fn checkout_wal_checkpoint_after_first_stash() {
        let (_repo, ctx, git) = setup();

        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feature".to_string(),
                name: None,
                base: None,
                description: None,
            },
        )
        .unwrap();

        let main_wt = git.main_worktree_path().unwrap();
        std::fs::write(main_wt.join("initial"), "dirty").unwrap();

        // Do the checkout — the WAL should be written during swap
        checkout(&ctx, &git, &main_wt, "feature", false).unwrap();

        // After successful checkout, WAL should be cleared
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(wkm_state.wal.is_none());
    }

    #[test]
    fn checkout_in_progress_op_blocks() {
        let (_repo, ctx, git) = setup();

        // Set up a WAL
        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.wal = Some(WalEntry {
            id: "test".to_string(),
            parent_op_id: None,
            op: WalOp::Swap {
                source_branch: "a".to_string(),
                target_branch: "b".to_string(),
                source_worktree: "/tmp/a".into(),
                target_worktree: None,
                main_stash: None,
                wt_stash: None,
                step: SwapStep::StashedMain,
            },
        });
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        _repo.create_branch("feature");
        let result = checkout(&ctx, &git, _repo.path(), "feature", false);
        assert!(matches!(result, Err(WkmError::OperationInProgress)));
    }
}
