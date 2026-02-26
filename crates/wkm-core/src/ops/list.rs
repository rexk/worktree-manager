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

/// Get the worktree path for a branch (for `wkm cd`).
pub fn cd_path(ctx: &RepoContext, branch: &str) -> Result<PathBuf, WkmError> {
    let wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    // Check if it's the base branch (main worktree)
    if branch == wkm_state.config.base_branch {
        return Ok(ctx.main_worktree.clone());
    }

    let entry = wkm_state
        .branches
        .get(branch)
        .ok_or_else(|| WkmError::BranchNotTracked(branch.to_string()))?;

    entry
        .worktree_path
        .clone()
        .ok_or_else(|| WkmError::NoWorktree(branch.to_string()))
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
        let (repo, ctx, _git) = setup();

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "feature".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: Some(repo.path().join("feature-wt")),
                stash_commit: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let path = cd_path(&ctx, "feature").unwrap();
        assert_eq!(path, repo.path().join("feature-wt"));
    }

    #[test]
    fn cd_base_branch_returns_main_worktree() {
        let (_repo, ctx, _git) = setup();
        let path = cd_path(&ctx, "main").unwrap();
        assert_eq!(path, ctx.main_worktree);
    }

    #[test]
    fn cd_no_worktree_errors() {
        let (_repo, ctx, _git) = setup();

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

        let result = cd_path(&ctx, "feature");
        assert!(matches!(result, Err(WkmError::NoWorktree(_))));
    }
}
