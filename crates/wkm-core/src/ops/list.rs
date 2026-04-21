use std::path::PathBuf;

use serde::Serialize;

use crate::error::WkmError;
use crate::git::{GitBranches, GitDiscovery};
use crate::repo::RepoContext;
use crate::state;

/// A branch entry in the list output.
#[derive(Debug, Clone, Serialize)]
pub struct ListEntry {
    pub name: String,
    pub parent: Option<String>,
    pub worktree_path: Option<PathBuf>,
    pub has_stash: bool,
    pub description: Option<String>,
    pub ahead_of_parent: Option<usize>,
    pub behind_parent: Option<usize>,
}

/// List all tracked branches.
pub fn list(
    ctx: &RepoContext,
    git: &(impl GitBranches + GitDiscovery),
) -> Result<Vec<ListEntry>, WkmError> {
    let wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    let mut entries = Vec::new();
    for (name, branch) in &wkm_state.branches {
        let (ahead, behind) = if let Some(ref parent) = branch.parent {
            if git.branch_exists(name)? && git.branch_exists(parent)? {
                let (a, b) = git.ahead_behind(name, parent)?;
                (Some(a), Some(b))
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        entries.push(ListEntry {
            name: name.clone(),
            parent: branch.parent.clone(),
            worktree_path: branch.worktree_path.clone(),
            has_stash: branch.stash_commit.is_some(),
            description: branch.description.clone(),
            ahead_of_parent: ahead,
            behind_parent: behind,
        });
    }
    Ok(entries)
}

/// Get the worktree path for a branch (for `wkm wp`).
///
/// Returns an error if the branch is not tracked, has no worktree assigned,
/// or the worktree directory no longer exists on disk.
pub fn cd_path(
    ctx: &RepoContext,
    git: &impl GitDiscovery,
    branch: &str,
) -> Result<PathBuf, WkmError> {
    let wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    // Check if it's the base branch (main worktree)
    if branch == wkm_state.config.base_branch {
        return Ok(ctx.main_worktree.clone());
    }

    // Any branch currently checked out in the main worktree lives there at
    // runtime — main-worktree hosting is inferred from git, not stored in state.
    if git.current_branch(&ctx.main_worktree)?.as_deref() == Some(branch) {
        return Ok(ctx.main_worktree.clone());
    }

    let entry = wkm_state
        .branches
        .get(branch)
        .ok_or_else(|| WkmError::BranchNotTracked(branch.to_string()))?;

    let path = entry
        .worktree_path
        .clone()
        .ok_or_else(|| WkmError::NoWorktree(branch.to_string()))?;

    if !path.exists() {
        return Err(WkmError::WorktreePathMissing(branch.to_string(), path));
    }

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::cli::CliGit;
    use crate::ops::init::{self, InitOptions};
    use crate::state::types::BranchEntry;
    use wkm_sandbox::TestRepo;

    fn setup() -> (TestRepo, RepoContext, CliGit) {
        let repo = TestRepo::new();
        let ctx = RepoContext::from_path(repo.path()).unwrap();
        let git = CliGit::new(repo.path());
        init::init(&ctx, &InitOptions::default()).unwrap();
        (repo, ctx, git)
    }

    #[test]
    fn list_shows_all_tracked() {
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
                jj_workspace_name: None,
                description: Some("A feature".to_string()),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let entries = list(&ctx, &git).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "feature");
        assert_eq!(entries[0].parent, Some("main".to_string()));
    }

    #[test]
    fn list_shows_stash_pending() {
        let (_repo, ctx, git) = setup();

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "feature".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: None,
                stash_commit: Some("abc123".to_string()),
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let entries = list(&ctx, &git).unwrap();
        assert!(entries[0].has_stash);
    }

    #[test]
    fn cd_returns_path() {
        let (repo, ctx, git) = setup();

        let wt_path = repo.path().join("feature-wt");
        std::fs::create_dir_all(&wt_path).unwrap();

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "feature".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: Some(wt_path.clone()),
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let path = cd_path(&ctx, &git, "feature").unwrap();
        assert_eq!(path, wt_path);
    }

    #[test]
    fn cd_base_branch_returns_main_worktree() {
        let (_repo, ctx, git) = setup();
        let path = cd_path(&ctx, &git, "main").unwrap();
        assert_eq!(path, ctx.main_worktree);
    }

    #[test]
    fn cd_path_missing_directory_errors() {
        let (_repo, ctx, git) = setup();

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "feature".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: Some(PathBuf::from("/tmp/nonexistent-worktree-path-12345")),
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let result = cd_path(&ctx, &git, "feature");
        assert!(
            matches!(result, Err(WkmError::WorktreePathMissing(ref b, _)) if b == "feature"),
            "expected WorktreePathMissing, got: {result:?}"
        );
    }

    #[test]
    fn cd_path_with_slash_in_branch_name() {
        let (repo, ctx, git) = setup();

        let wt_path = repo.path().join("cursor-wt");
        std::fs::create_dir_all(&wt_path).unwrap();

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "cursor/build-cache-cleanup-8644".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: Some(wt_path.clone()),
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let path = cd_path(&ctx, &git, "cursor/build-cache-cleanup-8644").unwrap();
        assert_eq!(path, wt_path);
    }

    #[test]
    fn cd_path_existing_directory_succeeds() {
        let (repo, ctx, git) = setup();

        let wt_path = repo.path().join("feature-wt");
        std::fs::create_dir_all(&wt_path).unwrap();

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "feature".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: Some(wt_path.clone()),
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let path = cd_path(&ctx, &git, "feature").unwrap();
        assert_eq!(path, wt_path);
    }

    #[test]
    fn cd_no_worktree_errors() {
        let (repo, ctx, git) = setup();

        // Create and checkout a different branch so `feature` is not hosted
        // in the main worktree — otherwise the runtime fallback would match.
        repo.create_branch("other");
        wkm_sandbox::git(repo.path(), &["checkout", "other"]);

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "feature".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: None,
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let result = cd_path(&ctx, &git, "feature");
        assert!(matches!(result, Err(WkmError::NoWorktree(_))));
    }

    #[test]
    fn cd_path_returns_main_for_branch_in_main_worktree() {
        let (repo, ctx, git) = setup();

        // Create a branch and check it out in the main worktree.
        repo.create_branch("hosted-in-main");
        wkm_sandbox::git(repo.path(), &["checkout", "hosted-in-main"]);

        // Tracked without a stored worktree_path (the new invariant).
        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "hosted-in-main".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: None,
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        // cd_path infers main-worktree hosting at runtime.
        let path = cd_path(&ctx, &git, "hosted-in-main").unwrap();
        assert_eq!(path, ctx.main_worktree);
    }
}
