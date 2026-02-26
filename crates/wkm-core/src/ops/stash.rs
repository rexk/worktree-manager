use crate::error::WkmError;
use crate::git::GitStash;
use crate::repo::RepoContext;
use crate::state;

/// A stash entry from wkm state.
pub struct WkmStashEntry {
    pub branch: String,
    pub commit: String,
}

/// List all wkm stashes tracked in state.
pub fn list(
    ctx: &RepoContext,
    branch_filter: Option<&str>,
) -> Result<Vec<WkmStashEntry>, WkmError> {
    let wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    let mut entries = Vec::new();
    for (name, entry) in &wkm_state.branches {
        if let Some(ref commit) = entry.stash_commit {
            if let Some(filter) = branch_filter {
                if name != filter {
                    continue;
                }
            }
            entries.push(WkmStashEntry {
                branch: name.clone(),
                commit: commit.clone(),
            });
        }
    }
    Ok(entries)
}

/// Apply a branch's stash without removing it from state.
pub fn apply(
    ctx: &RepoContext,
    git: &impl GitStash,
    branch: &str,
    worktree: &std::path::Path,
) -> Result<(), WkmError> {
    let wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    let entry = wkm_state
        .branches
        .get(branch)
        .ok_or_else(|| WkmError::BranchNotTracked(branch.to_string()))?;

    let commit = entry
        .stash_commit
        .as_ref()
        .ok_or_else(|| WkmError::Other(format!("no stash for branch '{branch}'")))?;

    git.stash_apply(worktree, commit, true)?;
    Ok(())
}

/// Drop a branch's stash from state (does not drop the git stash object).
pub fn drop(ctx: &RepoContext, branch: &str) -> Result<(), WkmError> {
    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    let entry = wkm_state
        .branches
        .get_mut(branch)
        .ok_or_else(|| WkmError::BranchNotTracked(branch.to_string()))?;

    if entry.stash_commit.is_none() {
        return Err(WkmError::Other(format!("no stash for branch '{branch}'")));
    }

    entry.stash_commit = None;
    state::write_state(&ctx.state_path, &wkm_state)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::cli::CliGit;
    use crate::git::{GitStash, GitStatus};
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

    fn add_branch_with_stash(ctx: &RepoContext, branch: &str, commit: &str) {
        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            branch.to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: None,
                stash_commit: Some(commit.to_string()),
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();
    }

    #[test]
    fn stash_list_all() {
        let (_repo, ctx, _git) = setup();
        add_branch_with_stash(&ctx, "feat-a", "abc123");
        add_branch_with_stash(&ctx, "feat-b", "def456");

        let entries = list(&ctx, None).unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn stash_list_by_branch() {
        let (_repo, ctx, _git) = setup();
        add_branch_with_stash(&ctx, "feat-a", "abc123");
        add_branch_with_stash(&ctx, "feat-b", "def456");

        let entries = list(&ctx, Some("feat-a")).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].branch, "feat-a");
    }

    #[test]
    fn stash_drop_removes_metadata_only() {
        let (_repo, ctx, _git) = setup();
        add_branch_with_stash(&ctx, "feat-a", "abc123");

        drop(&ctx, "feat-a").unwrap();

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(wkm_state.branches["feat-a"].stash_commit.is_none());
        // Branch still tracked
        assert!(wkm_state.branches.contains_key("feat-a"));
    }

    #[test]
    fn stash_drop_no_stash_errors() {
        let (_repo, ctx, _git) = setup();
        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "feat".to_string(),
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

        let result = drop(&ctx, "feat");
        assert!(result.is_err());
    }

    #[test]
    fn stash_apply_restores() {
        let (repo, ctx, git) = setup();

        // Create a real stash
        repo.make_dirty();
        let hash = git
            .stash_push(repo.path(), "wkm: test stash", false)
            .unwrap();

        add_branch_with_stash(&ctx, "feat", &hash);

        apply(&ctx, &git, "feat", repo.path()).unwrap();

        // Working tree should be dirty again
        assert!(git.is_dirty(repo.path()).unwrap());
    }

    #[test]
    fn stash_apply_not_tracked_errors() {
        let (_repo, ctx, git) = setup();
        let result = apply(&ctx, &git, "nonexistent", _repo.path());
        assert!(matches!(result, Err(WkmError::BranchNotTracked(_))));
    }
}
