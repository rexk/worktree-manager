use crate::error::WkmError;
use crate::git::GitBranches;
use crate::graph;
use crate::repo::RepoContext;
use crate::state;
use crate::state::lock::WkmLock;

/// Remove a branch from wkm tracking.
///
/// If `delete` is true, also delete the git branch.
pub fn drop(
    ctx: &RepoContext,
    git: &impl GitBranches,
    branch: &str,
    delete: bool,
) -> Result<Vec<String>, WkmError> {
    let lock = WkmLock::acquire(&ctx.lock_path)?;

    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    if wkm_state.wal.is_some() {
        return Err(WkmError::OperationInProgress);
    }

    // Cannot drop the base branch
    if branch == wkm_state.config.base_branch {
        return Err(WkmError::Other(format!(
            "cannot drop base branch '{branch}'"
        )));
    }

    // Must be tracked
    let entry = wkm_state
        .branches
        .get(branch)
        .ok_or_else(|| WkmError::BranchNotTracked(branch.to_string()))?;

    // Error if worktree still exists
    if entry.worktree_path.is_some() {
        return Err(WkmError::Other(format!(
            "branch '{branch}' still has a worktree. Remove it first: `wkm worktree remove {branch}`"
        )));
    }

    let dropped_parent = entry.parent.clone();

    // Re-parent children
    let children: Vec<String> = graph::children_of(branch, &wkm_state.branches)
        .iter()
        .map(|(name, _)| (*name).clone())
        .collect();

    for child_name in &children {
        if let Some(child_entry) = wkm_state.branches.get_mut(child_name) {
            child_entry.parent = dropped_parent.clone();
        }
    }

    // Clean up hold branch
    let hold_branch = format!("_wkm/hold/{branch}");
    if git.branch_exists(&hold_branch)? {
        let _ = git.delete_branch(&hold_branch, true);
    }

    // Remove from state
    wkm_state.branches.remove(branch);

    // Delete git branch if requested
    if delete {
        let _ = git.delete_branch(branch, true);
    }

    state::write_state(&ctx.state_path, &wkm_state)?;

    std::mem::drop(lock);
    Ok(children)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::cli::CliGit;
    use crate::ops::{adopt, init};
    use init::InitOptions;
    use wkm_sandbox::TestRepo;

    fn setup() -> (TestRepo, RepoContext, CliGit) {
        let repo = TestRepo::new();
        let ctx = RepoContext::from_path(repo.path()).unwrap();
        let git = CliGit::new(repo.path());
        init::init(&ctx, &InitOptions::default()).unwrap();
        (repo, ctx, git)
    }

    #[test]
    fn drop_removes_entry_keeps_git_branch() {
        let (repo, ctx, git) = setup();
        repo.create_branch("feature");
        adopt::adopt(&ctx, &git, &["feature".to_string()], None, false).unwrap();

        drop(&ctx, &git, "feature", false).unwrap();

        let state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(!state.branches.contains_key("feature"));
        assert!(git.branch_exists("feature").unwrap());
    }

    #[test]
    fn drop_delete_removes_git_branch() {
        let (repo, ctx, git) = setup();
        repo.create_branch("feature");
        adopt::adopt(&ctx, &git, &["feature".to_string()], None, false).unwrap();

        drop(&ctx, &git, "feature", true).unwrap();

        let state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(!state.branches.contains_key("feature"));
        assert!(!git.branch_exists("feature").unwrap());
    }

    #[test]
    fn drop_reparents_children() {
        let (repo, ctx, git) = setup();
        repo.create_branch("parent-feat");
        adopt::adopt(&ctx, &git, &["parent-feat".to_string()], None, false).unwrap();
        repo.create_branch("child-feat");
        adopt::adopt(
            &ctx,
            &git,
            &["child-feat".to_string()],
            Some("parent-feat"),
            false,
        )
        .unwrap();

        let reparented = drop(&ctx, &git, "parent-feat", false).unwrap();

        assert_eq!(reparented, vec!["child-feat".to_string()]);
        let state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert_eq!(
            state.branches["child-feat"].parent,
            Some("main".to_string())
        );
    }

    #[test]
    fn drop_errors_on_base_branch() {
        let (_repo, ctx, git) = setup();
        let result = drop(&ctx, &git, "main", false);
        assert!(matches!(result, Err(WkmError::Other(_))));
    }

    #[test]
    fn drop_errors_if_worktree_exists() {
        let (repo, ctx, git) = setup();
        repo.create_branch("wt-branch");
        adopt::adopt(&ctx, &git, &["wt-branch".to_string()], None, false).unwrap();

        // Manually set worktree_path in state
        let mut state = state::read_state(&ctx.state_path).unwrap().unwrap();
        state.branches.get_mut("wt-branch").unwrap().worktree_path = Some("/tmp/fake-wt".into());
        state::write_state(&ctx.state_path, &state).unwrap();

        let result = drop(&ctx, &git, "wt-branch", false);
        assert!(matches!(result, Err(WkmError::Other(_))));
    }
}
