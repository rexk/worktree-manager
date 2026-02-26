use crate::error::WkmError;
use crate::git::{GitBranches, GitDiscovery, GitWorktrees};
use crate::repo::RepoContext;
use crate::state;
use crate::state::lock::WkmLock;
use crate::state::types::BranchEntry;

/// Adopt an existing branch into wkm tracking.
pub fn adopt(
    ctx: &RepoContext,
    git: &(impl GitDiscovery + GitBranches + GitWorktrees),
    branch: &str,
    parent: Option<&str>,
) -> Result<(), WkmError> {
    let lock = WkmLock::acquire(&ctx.lock_path)?;

    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    if wkm_state.wal.is_some() {
        return Err(WkmError::OperationInProgress);
    }

    // Check branch exists
    if !git.branch_exists(branch)? {
        return Err(WkmError::BranchNotFound(branch.to_string()));
    }

    // Check not already tracked
    if wkm_state.branches.contains_key(branch) {
        return Err(WkmError::BranchAlreadyTracked(branch.to_string()));
    }

    // Determine parent
    let parent_branch = parent
        .map(|s| s.to_string())
        .or_else(|| git.current_branch(&ctx.main_worktree).ok().flatten())
        .unwrap_or_else(|| wkm_state.config.base_branch.clone());

    // Check if branch has a worktree
    let worktrees = git.worktree_list()?;
    let worktree_path = worktrees
        .iter()
        .find(|wt| wt.branch.as_deref() == Some(branch))
        .map(|wt| wt.path.clone());

    let now = chrono::Utc::now().to_rfc3339();
    wkm_state.branches.insert(
        branch.to_string(),
        BranchEntry {
            parent: Some(parent_branch),
            worktree_path,
            stash_commit: None,
            description: None,
            created_at: now,
            previous_branch: None,
        },
    );
    state::write_state(&ctx.state_path, &wkm_state)?;

    drop(lock);
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
    fn adopt_basic() {
        let (repo, ctx, git) = setup();
        repo.create_branch("existing-feature");
        adopt(&ctx, &git, "existing-feature", None).unwrap();

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(wkm_state.branches.contains_key("existing-feature"));
        assert_eq!(
            wkm_state.branches["existing-feature"].parent,
            Some("main".to_string())
        );
    }

    #[test]
    fn adopt_explicit_parent() {
        let (repo, ctx, git) = setup();
        repo.create_branch("develop");
        repo.create_branch("feature");
        adopt(&ctx, &git, "feature", Some("develop")).unwrap();

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert_eq!(
            wkm_state.branches["feature"].parent,
            Some("develop".to_string())
        );
    }

    #[test]
    fn adopt_detects_worktree() {
        let (repo, ctx, git) = setup();
        repo.create_branch("wt-branch");

        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("wt");
        wkm_sandbox::git(
            repo.path(),
            &["worktree", "add", wt_path.to_str().unwrap(), "wt-branch"],
        );

        adopt(&ctx, &git, "wt-branch", None).unwrap();

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(wkm_state.branches["wt-branch"].worktree_path.is_some());

        // Cleanup
        wkm_sandbox::git(
            repo.path(),
            &["worktree", "remove", wt_path.to_str().unwrap()],
        );
    }

    #[test]
    fn adopt_nonexistent_errors() {
        let (_repo, ctx, git) = setup();
        let result = adopt(&ctx, &git, "nonexistent", None);
        assert!(matches!(result, Err(WkmError::BranchNotFound(_))));
    }

    #[test]
    fn adopt_already_tracked_errors() {
        let (repo, ctx, git) = setup();
        repo.create_branch("feature");
        adopt(&ctx, &git, "feature", None).unwrap();
        let result = adopt(&ctx, &git, "feature", None);
        assert!(matches!(result, Err(WkmError::BranchAlreadyTracked(_))));
    }
}
