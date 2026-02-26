use serde::Serialize;

use crate::error::WkmError;
use crate::git::{GitBranches, GitDiscovery, GitStatus as GitStatusTrait};
use crate::repo::RepoContext;
use crate::state;
use crate::state::types::WalOp;

/// Status information for the current branch.
#[derive(Debug, Clone, Serialize)]
pub struct BranchStatus {
    pub branch: String,
    pub parent: Option<String>,
    pub ahead_of_parent: Option<usize>,
    pub behind_parent: Option<usize>,
    pub ahead_of_remote: Option<usize>,
    pub behind_remote: Option<usize>,
    pub is_dirty: bool,
    pub in_progress_op: Option<String>,
}

/// Get status for the current branch.
pub fn status(
    ctx: &RepoContext,
    git: &(impl GitDiscovery + GitBranches + GitStatusTrait),
    worktree: &std::path::Path,
) -> Result<BranchStatus, WkmError> {
    let wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    let branch_name = git
        .current_branch(worktree)?
        .ok_or_else(|| WkmError::Other("detached HEAD".to_string()))?;

    let entry = wkm_state.branches.get(&branch_name);

    let parent = entry.and_then(|e| e.parent.clone());

    let (ahead_parent, behind_parent) = if let Some(ref p) = parent {
        if git.branch_exists(&branch_name)? && git.branch_exists(p)? {
            let (a, b) = git.ahead_behind(&branch_name, p)?;
            (Some(a), Some(b))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let (ahead_remote, behind_remote) =
        if let Ok(Some(upstream)) = git.remote_tracking_branch(&branch_name) {
            if git.branch_exists(&branch_name)? {
                let (a, b) = git.ahead_behind(&branch_name, &upstream)?;
                (Some(a), Some(b))
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

    let is_dirty = git.is_dirty(worktree)?;

    let in_progress_op = wkm_state.wal.as_ref().map(|wal| match &wal.op {
        WalOp::Swap { .. } => "swap".to_string(),
        WalOp::Sync { conflicted, .. } => {
            if let Some(branch) = conflicted {
                format!("sync (conflict in {branch})")
            } else {
                "sync".to_string()
            }
        }
        WalOp::Merge { child_branch, .. } => format!("merge ({child_branch})"),
        WalOp::MergeAll { .. } => "merge --all".to_string(),
    });

    Ok(BranchStatus {
        branch: branch_name,
        parent,
        ahead_of_parent: ahead_parent,
        behind_parent: behind_parent,
        ahead_of_remote: ahead_remote,
        behind_remote: behind_remote,
        is_dirty,
        in_progress_op,
    })
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
    fn status_ahead_behind_parent() {
        let (repo, ctx, git) = setup();
        repo.create_branch("feature");
        repo.checkout("feature");
        repo.commit_file("f1", "data", "feature commit");

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

        let s = status(&ctx, &git, repo.path()).unwrap();
        assert_eq!(s.branch, "feature");
        assert_eq!(s.ahead_of_parent, Some(1));
        assert_eq!(s.behind_parent, Some(0));
    }

    #[test]
    fn status_in_progress_op() {
        let (_repo, ctx, git) = setup();

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.wal = Some(crate::state::types::WalEntry {
            id: "test".to_string(),
            parent_op_id: None,
            op: WalOp::Sync {
                pre_refs: Default::default(),
                completed: vec![],
                conflicted: Some("feat".to_string()),
                pending: vec![],
                temp_worktrees: vec![],
            },
        });
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let s = status(&ctx, &git, _repo.path()).unwrap();
        assert!(s.in_progress_op.unwrap().contains("conflict in feat"));
    }
}
