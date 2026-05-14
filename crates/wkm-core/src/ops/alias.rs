use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::encoding;
use crate::error::WkmError;
use crate::repo::RepoContext;
use crate::state;
use crate::state::lock::WkmLock;
use crate::state::types::AliasEntry;

/// One entry in the `wkm alias list` output.
#[derive(Debug, Clone, Serialize)]
pub struct AliasInfo {
    pub alias: String,
    pub worktree_path: PathBuf,
    pub description: Option<String>,
    pub created_at: String,
    /// `true` when the stored directory no longer exists on disk.
    pub stale: bool,
    /// The branch currently checked out in the aliased worktree (if any).
    pub current_branch: Option<String>,
}

/// Where a new alias should point.
pub enum AliasTarget<'a> {
    /// A tracked branch name — resolves to that branch's secondary worktree.
    Branch(&'a str),
    /// An explicit directory path (typically `cwd`).
    Path(&'a Path),
}

/// List all registered aliases.
pub fn list(
    ctx: &RepoContext,
    git: &impl crate::git::GitDiscovery,
) -> Result<Vec<AliasInfo>, WkmError> {
    let wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    let mut out = Vec::with_capacity(wkm_state.aliases.len());
    for (alias, entry) in &wkm_state.aliases {
        let stale = !entry.worktree_path.exists();
        let current_branch = if stale {
            None
        } else {
            git.current_branch(&entry.worktree_path).ok().flatten()
        };
        out.push(AliasInfo {
            alias: alias.clone(),
            worktree_path: entry.worktree_path.clone(),
            description: entry.description.clone(),
            created_at: entry.created_at.clone(),
            stale,
            current_branch,
        });
    }
    Ok(out)
}

/// Create or reuse an alias pointing at the given target.
///
/// Errors if:
/// - the alias name fails validation,
/// - the target is the main worktree,
/// - the resolved path isn't a known secondary worktree,
/// - another alias already points somewhere else.
///
/// For `AliasTarget::Path`, when cwd is a real git worktree but the wkm state
/// has the wrong path recorded for its branch, the error explicitly tells the
/// user to run `wkm repair` rather than parroting the invariant.
pub fn set(
    ctx: &RepoContext,
    git: &(impl crate::git::GitDiscovery + crate::git::GitWorktrees),
    alias: &str,
    target: AliasTarget<'_>,
) -> Result<(), WkmError> {
    encoding::validate_alias(alias).map_err(WkmError::InvalidAlias)?;

    let lock = WkmLock::acquire(&ctx.lock_path)?;
    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    if wkm_state.wal.is_some() {
        return Err(WkmError::OperationInProgress);
    }

    let path = match target {
        AliasTarget::Branch(branch) => {
            let entry = wkm_state
                .branches
                .get(branch)
                .ok_or_else(|| WkmError::BranchNotTracked(branch.to_string()))?;
            entry
                .worktree_path
                .clone()
                .ok_or_else(|| WkmError::NoWorktree(branch.to_string()))?
        }
        AliasTarget::Path(p) => resolve_path_target(ctx, git, &wkm_state, p)?,
    };

    if !path.exists() {
        return Err(WkmError::AliasPathMissing(alias.to_string(), path));
    }

    // Same alias already pointing elsewhere? Reject.
    if let Some(existing) = wkm_state.aliases.get(alias)
        && existing.worktree_path != path
    {
        return Err(WkmError::AliasExists(
            alias.to_string(),
            existing.worktree_path.clone(),
        ));
    }

    // Another alias already owns this path? Reject (one alias per worktree).
    if let Some((other_alias, _)) = wkm_state
        .aliases
        .iter()
        .find(|(k, v)| k.as_str() != alias && v.worktree_path == path)
    {
        return Err(WkmError::Other(format!(
            "path already aliased as '{other_alias}' — clear or rename that alias first"
        )));
    }

    let now = chrono::Utc::now().to_rfc3339();
    wkm_state.aliases.insert(
        alias.to_string(),
        AliasEntry {
            worktree_path: path,
            created_at: now,
            description: None,
        },
    );
    state::write_state(&ctx.state_path, &wkm_state)?;
    drop(lock);
    Ok(())
}

/// Resolve `AliasTarget::Path` to the path that should be stored.
///
/// On success, returns a path that matches a tracked branch's recorded
/// `worktree_path`. On failure, the error is one of four actionable
/// diagnostics:
///
/// - cwd is (inside) the main worktree → use `@main`.
/// - cwd is a tracked branch's worktree but state has the wrong path → stale state, run `wkm repair`.
/// - cwd is some other branch's git worktree → not tracked, run `wkm adopt`.
/// - cwd isn't a git worktree at all → create or pass `--branch`.
///
/// Path equality is checked against canonicalized forms on both sides
/// because on Windows `Path::canonicalize` produces `\\?\` UNC paths while
/// `git worktree list` reports forward-slash drive paths.
fn resolve_path_target(
    ctx: &RepoContext,
    git: &(impl crate::git::GitDiscovery + crate::git::GitWorktrees),
    wkm_state: &crate::state::types::WkmState,
    p: &Path,
) -> Result<PathBuf, WkmError> {
    let canon = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let main_canon = ctx
        .main_worktree
        .canonicalize()
        .unwrap_or_else(|_| ctx.main_worktree.clone());
    if canon == main_canon || canon.starts_with(&main_canon) {
        return Err(WkmError::Other(
            "cannot alias the main worktree (use '@main' to navigate there)".to_string(),
        ));
    }

    let worktrees = git.worktree_list().unwrap_or_default();
    let wt_at_canon = worktrees.iter().find(|wt| {
        wt.path
            .canonicalize()
            .map(|c| c == canon)
            .unwrap_or_else(|_| wt.path == canon)
    });

    let Some(wt) = wt_at_canon else {
        return Err(WkmError::Other(format!(
            "no tracked branch has a worktree at {}. Create one with `wkm worktree create` or pass --branch <name>.",
            canon.display()
        )));
    };

    let Some(branch) = wt.branch.as_deref() else {
        return Err(WkmError::Other(format!(
            "worktree at {} is in detached HEAD state; aliases require a branch.",
            canon.display()
        )));
    };

    let Some(entry) = wkm_state.branches.get(branch) else {
        return Err(WkmError::Other(format!(
            "branch '{branch}' at {} is not tracked by wkm. Run `wkm adopt {branch}` first.",
            canon.display()
        )));
    };

    let state_matches = entry
        .worktree_path
        .as_deref()
        .and_then(|p| p.canonicalize().ok())
        .is_some_and(|c| c == canon);
    if state_matches {
        Ok(canon)
    } else {
        let recorded = match &entry.worktree_path {
            Some(p) => p.display().to_string(),
            None => "<none>".to_string(),
        };
        Err(WkmError::Other(format!(
            "wkm state is stale for branch '{branch}': recorded worktree_path = {recorded}, but it actually lives at {}. Run `wkm repair` and retry.",
            canon.display()
        )))
    }
}
/// Rename an alias. Errors if `old` is unknown or `new` is already taken.
pub fn rename(ctx: &RepoContext, old: &str, new: &str) -> Result<(), WkmError> {
    encoding::validate_alias(new).map_err(WkmError::InvalidAlias)?;

    let lock = WkmLock::acquire(&ctx.lock_path)?;
    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    if wkm_state.wal.is_some() {
        return Err(WkmError::OperationInProgress);
    }

    if old == new {
        return Ok(());
    }

    if wkm_state.aliases.contains_key(new) {
        let existing = &wkm_state.aliases[new];
        return Err(WkmError::AliasExists(
            new.to_string(),
            existing.worktree_path.clone(),
        ));
    }

    let entry = wkm_state
        .aliases
        .remove(old)
        .ok_or_else(|| WkmError::AliasNotFound(old.to_string()))?;
    wkm_state.aliases.insert(new.to_string(), entry);
    state::write_state(&ctx.state_path, &wkm_state)?;
    drop(lock);
    Ok(())
}

/// Remove an alias. The underlying worktree directory is unaffected.
pub fn clear(ctx: &RepoContext, alias: &str) -> Result<(), WkmError> {
    let lock = WkmLock::acquire(&ctx.lock_path)?;
    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    if wkm_state.wal.is_some() {
        return Err(WkmError::OperationInProgress);
    }

    wkm_state
        .aliases
        .remove(alias)
        .ok_or_else(|| WkmError::AliasNotFound(alias.to_string()))?;
    state::write_state(&ctx.state_path, &wkm_state)?;
    drop(lock);
    Ok(())
}

/// Return the alias pointing at `path`, if any.
pub fn alias_for_path(wkm_state: &crate::state::types::WkmState, path: &Path) -> Option<String> {
    wkm_state
        .aliases
        .iter()
        .find(|(_, v)| v.worktree_path == path)
        .map(|(k, _)| k.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::cli::CliGit;
    use crate::ops::init::{self, InitOptions};
    use crate::ops::worktree::{self, CreateOptions};
    use wkm_sandbox::TestRepo;

    fn setup() -> (TestRepo, RepoContext, CliGit) {
        let repo = TestRepo::new();
        let ctx = RepoContext::from_path(repo.path()).unwrap();
        let git = CliGit::new(repo.path());
        init::init(&ctx, &InitOptions::default()).unwrap();
        (repo, ctx, git)
    }

    #[test]
    fn set_by_branch_registers_alias() {
        let (_repo, ctx, git) = setup();
        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feat".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();

        set(&ctx, &git, "specs", AliasTarget::Branch("feat")).unwrap();

        let state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(state.aliases.contains_key("specs"));
    }

    #[test]
    fn set_rejects_invalid_alias() {
        let (_repo, ctx, git) = setup();
        let err = set(&ctx, &git, "@main", AliasTarget::Branch("anything"));
        assert!(matches!(err, Err(WkmError::InvalidAlias(_))));
    }

    #[test]
    fn set_rejects_main_worktree_path() {
        let (_repo, ctx, git) = setup();
        let main = ctx.main_worktree.clone();
        let err = set(&ctx, &git, "home", AliasTarget::Path(&main));
        assert!(err.is_err());
    }

    #[test]
    fn set_rejects_duplicate_alias_elsewhere() {
        let (_repo, ctx, git) = setup();
        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feat-a".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();
        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feat-b".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();

        set(&ctx, &git, "specs", AliasTarget::Branch("feat-a")).unwrap();
        let err = set(&ctx, &git, "specs", AliasTarget::Branch("feat-b"));
        assert!(matches!(err, Err(WkmError::AliasExists(_, _))));
    }

    #[test]
    fn rename_moves_alias() {
        let (_repo, ctx, git) = setup();
        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feat".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();
        set(&ctx, &git, "specs", AliasTarget::Branch("feat")).unwrap();

        rename(&ctx, "specs", "scratch").unwrap();

        let state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(!state.aliases.contains_key("specs"));
        assert!(state.aliases.contains_key("scratch"));
    }

    #[test]
    fn rename_rejects_existing_target() {
        let (_repo, ctx, git) = setup();
        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feat-a".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();
        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feat-b".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();
        set(&ctx, &git, "a", AliasTarget::Branch("feat-a")).unwrap();
        set(&ctx, &git, "b", AliasTarget::Branch("feat-b")).unwrap();

        let err = rename(&ctx, "a", "b");
        assert!(matches!(err, Err(WkmError::AliasExists(_, _))));
    }

    #[test]
    fn clear_removes_alias_keeps_worktree() {
        let (_repo, ctx, git) = setup();
        let created = worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feat".to_string(),
                base: None,
                description: None,
                name: Some("specs".to_string()),
            },
        )
        .unwrap();

        assert!(created.worktree_path.exists());

        clear(&ctx, "specs").unwrap();

        let state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(!state.aliases.contains_key("specs"));
        assert!(created.worktree_path.exists());
    }

    #[test]
    fn clear_unknown_errors() {
        let (_repo, ctx, _git) = setup();
        let err = clear(&ctx, "missing");
        assert!(matches!(err, Err(WkmError::AliasNotFound(_))));
    }

    #[test]
    fn list_marks_stale_when_path_gone() {
        let (_repo, ctx, git) = setup();
        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feat".to_string(),
                base: None,
                description: None,
                name: Some("specs".to_string()),
            },
        )
        .unwrap();

        // Simulate the path going away out-of-band.
        let mut state = state::read_state(&ctx.state_path).unwrap().unwrap();
        state.aliases.get_mut("specs").unwrap().worktree_path =
            PathBuf::from("/tmp/wkm-nonexistent-alias-path-99999");
        state::write_state(&ctx.state_path, &state).unwrap();

        let rows = list(&ctx, &git).unwrap();
        assert!(rows.iter().any(|w| w.alias == "specs" && w.stale));
    }

    #[test]
    fn set_from_path_reports_stale_state_when_state_disagrees_with_git() {
        // Create a tracked branch with a real secondary worktree, then poison
        // the recorded `worktree_path` so it disagrees with git. Calling `set`
        // from inside the actual worktree should produce a "stale state, run
        // `wkm repair`" error — not the generic "not a wkm-managed worktree".
        let (_repo, ctx, git) = setup();
        let created = worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feat".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();

        let mut state = state::read_state(&ctx.state_path).unwrap().unwrap();
        state.branches.get_mut("feat").unwrap().worktree_path =
            Some(PathBuf::from("/tmp/wkm-stale-path-xyz"));
        state::write_state(&ctx.state_path, &state).unwrap();

        let err = set(
            &ctx,
            &git,
            "specs",
            AliasTarget::Path(&created.worktree_path),
        );
        let msg = match err {
            Err(WkmError::Other(m)) => m,
            other => panic!("expected Other error, got {other:?}"),
        };
        assert!(
            msg.contains("stale") && msg.contains("wkm repair"),
            "error should mention stale state and `wkm repair`, got: {msg}"
        );
    }

    #[test]
    fn set_from_path_reports_untracked_branch() {
        // Create a worktree manually (bypassing wkm) and run set from inside.
        // The diagnostic should say "not tracked, run `wkm adopt`".
        let (_repo, ctx, git) = setup();
        let scratch_root = tempfile::tempdir().unwrap();
        let scratch_dir = scratch_root.path().join("untracked-wt");
        wkm_sandbox::git(
            &ctx.main_worktree,
            &[
                "worktree",
                "add",
                "-b",
                "feat-untracked",
                scratch_dir.to_str().unwrap(),
            ],
        );
        let canon = scratch_dir.canonicalize().unwrap_or(scratch_dir.clone());

        let err = set(&ctx, &git, "tag", AliasTarget::Path(&canon));
        let msg = match err {
            Err(WkmError::Other(m)) => m,
            other => panic!("expected Other error, got {other:?}"),
        };
        assert!(
            msg.contains("not tracked by wkm") && msg.contains("wkm adopt"),
            "error should mention adopt, got: {msg}"
        );
    }

    #[test]
    fn set_from_random_path_reports_no_worktree() {
        let (_repo, ctx, git) = setup();
        let tmp = tempfile::tempdir().unwrap();
        let err = set(&ctx, &git, "tag", AliasTarget::Path(tmp.path()));
        let msg = match err {
            Err(WkmError::Other(m)) => m,
            other => panic!("expected Other error, got {other:?}"),
        };
        assert!(
            msg.contains("no tracked branch has a worktree at"),
            "error should mention no tracked worktree at the path, got: {msg}"
        );
    }
}
