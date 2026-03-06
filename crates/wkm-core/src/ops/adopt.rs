use crate::error::WkmError;
use crate::git::{GitBranches, GitDiscovery, GitWorktrees};
use crate::repo::RepoContext;
use crate::state;
use crate::state::lock::WkmLock;
use crate::state::types::BranchEntry;

pub struct AdoptResult {
    pub adopted: Vec<String>,
    pub skipped: Vec<String>,
}

/// Adopt one or more existing branches into wkm tracking.
///
/// When `lenient` is true (used by `--all`), already-tracked branches are
/// silently skipped. When false, they produce an error.
pub fn adopt(
    ctx: &RepoContext,
    git: &(impl GitDiscovery + GitBranches + GitWorktrees),
    branches: &[String],
    parent: Option<&str>,
    lenient: bool,
) -> Result<AdoptResult, WkmError> {
    let lock = WkmLock::acquire(&ctx.lock_path)?;

    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    if wkm_state.wal.is_some() {
        return Err(WkmError::OperationInProgress);
    }

    // Resolve parent once for all branches
    let parent_branch = parent
        .map(|s| s.to_string())
        .or_else(|| git.current_branch(&ctx.main_worktree).ok().flatten())
        .unwrap_or_else(|| wkm_state.config.base_branch.clone());

    // Fetch worktree list once
    let worktrees = git.worktree_list()?;

    let mut adopted = Vec::new();
    let mut skipped = Vec::new();

    // Validate all branches first (all-or-nothing for non-lenient errors)
    for branch in branches {
        if !git.branch_exists(branch)? {
            return Err(WkmError::BranchNotFound(branch.clone()));
        }
        if wkm_state.branches.contains_key(branch.as_str()) {
            if lenient {
                continue;
            } else {
                return Err(WkmError::BranchAlreadyTracked(branch.clone()));
            }
        }
    }

    // Now insert all branches
    let now = chrono::Utc::now().to_rfc3339();
    for branch in branches {
        if wkm_state.branches.contains_key(branch.as_str()) {
            skipped.push(branch.clone());
            continue;
        }

        let worktree_path = worktrees
            .iter()
            .find(|wt| wt.branch.as_deref() == Some(branch.as_str()))
            .map(|wt| wt.path.clone());

        wkm_state.branches.insert(
            branch.clone(),
            BranchEntry {
                parent: Some(parent_branch.clone()),
                worktree_path,
                stash_commit: None,
                description: None,
                created_at: now.clone(),
                previous_branch: None,
            },
        );
        adopted.push(branch.clone());
    }

    if !adopted.is_empty() {
        state::write_state(&ctx.state_path, &wkm_state)?;
    }

    drop(lock);
    Ok(AdoptResult { adopted, skipped })
}

/// Discover branches not yet tracked by wkm.
///
/// Filters out: the base branch, `_wkm/*` internal branches, and
/// already-tracked branches.
pub fn discover_untracked(
    ctx: &RepoContext,
    git: &(impl GitBranches + GitDiscovery),
) -> Result<Vec<String>, WkmError> {
    let wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;
    let all_branches = git.branch_list()?;
    let base = &wkm_state.config.base_branch;

    Ok(all_branches
        .into_iter()
        .filter(|b| {
            b != base && !b.starts_with("_wkm/") && !wkm_state.branches.contains_key(b.as_str())
        })
        .collect())
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
        let result = adopt(&ctx, &git, &["existing-feature".to_string()], None, false).unwrap();

        assert_eq!(result.adopted, vec!["existing-feature"]);
        assert!(result.skipped.is_empty());

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
        let result = adopt(&ctx, &git, &["feature".to_string()], Some("develop"), false).unwrap();

        assert_eq!(result.adopted, vec!["feature"]);
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

        adopt(&ctx, &git, &["wt-branch".to_string()], None, false).unwrap();

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
        let result = adopt(&ctx, &git, &["nonexistent".to_string()], None, false);
        assert!(matches!(result, Err(WkmError::BranchNotFound(_))));
    }

    #[test]
    fn adopt_already_tracked_errors() {
        let (repo, ctx, git) = setup();
        repo.create_branch("feature");
        adopt(&ctx, &git, &["feature".to_string()], None, false).unwrap();
        let result = adopt(&ctx, &git, &["feature".to_string()], None, false);
        assert!(matches!(result, Err(WkmError::BranchAlreadyTracked(_))));
    }

    #[test]
    fn adopt_batch_multiple() {
        let (repo, ctx, git) = setup();
        repo.create_branch("alpha");
        repo.create_branch("beta");
        repo.create_branch("gamma");

        let result = adopt(
            &ctx,
            &git,
            &["alpha".to_string(), "beta".to_string(), "gamma".to_string()],
            None,
            false,
        )
        .unwrap();

        assert_eq!(result.adopted, vec!["alpha", "beta", "gamma"]);
        assert!(result.skipped.is_empty());

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(wkm_state.branches.contains_key("alpha"));
        assert!(wkm_state.branches.contains_key("beta"));
        assert!(wkm_state.branches.contains_key("gamma"));
    }

    #[test]
    fn adopt_batch_all_or_nothing() {
        let (repo, ctx, git) = setup();
        repo.create_branch("good");
        // "bad" doesn't exist

        let result = adopt(
            &ctx,
            &git,
            &["good".to_string(), "bad".to_string()],
            None,
            false,
        );
        assert!(matches!(result, Err(WkmError::BranchNotFound(_))));

        // "good" should NOT have been persisted
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(!wkm_state.branches.contains_key("good"));
    }

    #[test]
    fn adopt_lenient_skips_tracked() {
        let (repo, ctx, git) = setup();
        repo.create_branch("tracked");
        repo.create_branch("untracked");
        adopt(&ctx, &git, &["tracked".to_string()], None, false).unwrap();

        let result = adopt(
            &ctx,
            &git,
            &["tracked".to_string(), "untracked".to_string()],
            None,
            true,
        )
        .unwrap();

        assert_eq!(result.adopted, vec!["untracked"]);
        assert_eq!(result.skipped, vec!["tracked"]);
    }

    #[test]
    fn discover_untracked_filters() {
        let (repo, ctx, git) = setup();
        repo.create_branch("feature-a");
        repo.create_branch("feature-b");
        repo.create_branch("_wkm/internal");

        // Track feature-a
        adopt(&ctx, &git, &["feature-a".to_string()], None, false).unwrap();

        let untracked = discover_untracked(&ctx, &git).unwrap();
        assert_eq!(untracked, vec!["feature-b"]);
    }
}
