use crate::error::WkmError;
use crate::git::GitBranches;
use crate::graph;
use crate::repo::RepoContext;
use crate::state;
use crate::state::lock::WkmLock;

pub struct SetParentResult {
    pub branch: String,
    pub old_parent: Option<String>,
    pub new_parent: String,
}

/// Change the tracked parent of a branch.
///
/// This is a metadata-only operation (updates `BranchEntry.parent` in state).
/// The caller should run `sync` afterward to rebase the branch graph.
pub fn set_parent(
    ctx: &RepoContext,
    git: &impl GitBranches,
    branch: &str,
    new_parent: &str,
) -> Result<SetParentResult, WkmError> {
    let lock = WkmLock::acquire(&ctx.lock_path)?;

    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    if wkm_state.wal.is_some() {
        return Err(WkmError::OperationInProgress);
    }

    // Cannot reparent the base branch
    if branch == wkm_state.config.base_branch {
        return Err(WkmError::Other(format!(
            "cannot change parent of base branch '{branch}'"
        )));
    }

    // Target branch must be tracked
    let entry = wkm_state
        .branches
        .get(branch)
        .ok_or_else(|| WkmError::BranchNotTracked(branch.to_string()))?;

    let old_parent = entry.parent.clone();

    // No-op if already the current parent
    if old_parent.as_deref() == Some(new_parent) {
        std::mem::drop(lock);
        return Ok(SetParentResult {
            branch: branch.to_string(),
            old_parent,
            new_parent: new_parent.to_string(),
        });
    }

    // New parent must exist in git
    if !git.branch_exists(new_parent)? {
        return Err(WkmError::BranchNotFound(new_parent.to_string()));
    }

    // New parent must be tracked or be the base branch
    if new_parent != wkm_state.config.base_branch && !wkm_state.branches.contains_key(new_parent) {
        return Err(WkmError::BranchNotTracked(new_parent.to_string()));
    }

    // No self-reparenting
    if new_parent == branch {
        return Err(WkmError::Other(
            "cannot set a branch as its own parent".to_string(),
        ));
    }

    // No cycles: new parent must not be a descendant of the branch
    let descendants = graph::descendants_of(branch, &wkm_state.branches);
    if descendants
        .iter()
        .any(|(name, _)| name.as_str() == new_parent)
    {
        return Err(WkmError::Other(format!(
            "reparenting '{branch}' to '{new_parent}' would create a cycle"
        )));
    }

    // Update parent
    wkm_state.branches.get_mut(branch).unwrap().parent = Some(new_parent.to_string());

    state::write_state(&ctx.state_path, &wkm_state)?;

    std::mem::drop(lock);
    Ok(SetParentResult {
        branch: branch.to_string(),
        old_parent,
        new_parent: new_parent.to_string(),
    })
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
    fn set_parent_basic() {
        let (repo, ctx, git) = setup();
        repo.create_branch("develop");
        adopt::adopt(&ctx, &git, &["develop".to_string()], None, false).unwrap();
        repo.create_branch("feature");
        adopt::adopt(&ctx, &git, &["feature".to_string()], None, false).unwrap();

        let result = set_parent(&ctx, &git, "feature", "develop").unwrap();

        assert_eq!(result.branch, "feature");
        assert_eq!(result.old_parent, Some("main".to_string()));
        assert_eq!(result.new_parent, "develop");

        let state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert_eq!(
            state.branches["feature"].parent,
            Some("develop".to_string())
        );
    }

    #[test]
    fn set_parent_cycle_detection() {
        let (repo, ctx, git) = setup();
        repo.create_branch("a");
        adopt::adopt(&ctx, &git, &["a".to_string()], None, false).unwrap();
        repo.create_branch("b");
        adopt::adopt(&ctx, &git, &["b".to_string()], Some("a"), false).unwrap();

        let result = set_parent(&ctx, &git, "a", "b");
        assert!(matches!(result, Err(WkmError::Other(ref msg)) if msg.contains("cycle")));
    }

    #[test]
    fn set_parent_deep_cycle() {
        let (repo, ctx, git) = setup();
        repo.create_branch("a");
        adopt::adopt(&ctx, &git, &["a".to_string()], None, false).unwrap();
        repo.create_branch("b");
        adopt::adopt(&ctx, &git, &["b".to_string()], Some("a"), false).unwrap();
        repo.create_branch("c");
        adopt::adopt(&ctx, &git, &["c".to_string()], Some("b"), false).unwrap();

        let result = set_parent(&ctx, &git, "a", "c");
        assert!(matches!(result, Err(WkmError::Other(ref msg)) if msg.contains("cycle")));
    }

    #[test]
    fn set_parent_self_loop() {
        let (repo, ctx, git) = setup();
        repo.create_branch("feature");
        adopt::adopt(&ctx, &git, &["feature".to_string()], None, false).unwrap();

        let result = set_parent(&ctx, &git, "feature", "feature");
        assert!(matches!(result, Err(WkmError::Other(ref msg)) if msg.contains("its own parent")));
    }

    #[test]
    fn set_parent_to_base_branch() {
        let (repo, ctx, git) = setup();
        repo.create_branch("develop");
        adopt::adopt(&ctx, &git, &["develop".to_string()], None, false).unwrap();
        repo.create_branch("feature");
        adopt::adopt(&ctx, &git, &["feature".to_string()], Some("develop"), false).unwrap();

        let result = set_parent(&ctx, &git, "feature", "main").unwrap();
        assert_eq!(result.new_parent, "main");

        let state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert_eq!(state.branches["feature"].parent, Some("main".to_string()));
    }

    #[test]
    fn set_parent_untracked_branch() {
        let (repo, ctx, git) = setup();
        repo.create_branch("feature");
        // Don't adopt — not tracked

        let result = set_parent(&ctx, &git, "feature", "main");
        assert!(matches!(result, Err(WkmError::BranchNotTracked(_))));
    }

    #[test]
    fn set_parent_untracked_parent() {
        let (repo, ctx, git) = setup();
        repo.create_branch("feature");
        adopt::adopt(&ctx, &git, &["feature".to_string()], None, false).unwrap();
        repo.create_branch("untracked");

        let result = set_parent(&ctx, &git, "feature", "untracked");
        assert!(matches!(result, Err(WkmError::BranchNotTracked(_))));
    }

    #[test]
    fn set_parent_nonexistent_parent() {
        let (repo, ctx, git) = setup();
        repo.create_branch("feature");
        adopt::adopt(&ctx, &git, &["feature".to_string()], None, false).unwrap();

        let result = set_parent(&ctx, &git, "feature", "does-not-exist");
        assert!(matches!(result, Err(WkmError::BranchNotFound(_))));
    }

    #[test]
    fn set_parent_cannot_reparent_base() {
        let (repo, ctx, git) = setup();
        repo.create_branch("develop");
        adopt::adopt(&ctx, &git, &["develop".to_string()], None, false).unwrap();

        let result = set_parent(&ctx, &git, "main", "develop");
        assert!(matches!(result, Err(WkmError::Other(ref msg)) if msg.contains("base branch")));
    }

    #[test]
    fn set_parent_noop_same_parent() {
        let (repo, ctx, git) = setup();
        repo.create_branch("feature");
        adopt::adopt(&ctx, &git, &["feature".to_string()], None, false).unwrap();

        let result = set_parent(&ctx, &git, "feature", "main").unwrap();
        assert_eq!(result.old_parent, Some("main".to_string()));
        assert_eq!(result.new_parent, "main");
    }

    #[test]
    fn set_parent_preserves_children() {
        let (repo, ctx, git) = setup();
        repo.create_branch("develop");
        adopt::adopt(&ctx, &git, &["develop".to_string()], None, false).unwrap();
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

        set_parent(&ctx, &git, "parent-feat", "develop").unwrap();

        let state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert_eq!(
            state.branches["parent-feat"].parent,
            Some("develop".to_string())
        );
        // Child still points to parent-feat, not to the old parent
        assert_eq!(
            state.branches["child-feat"].parent,
            Some("parent-feat".to_string())
        );
    }
}
