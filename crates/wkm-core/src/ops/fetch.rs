use crate::error::WkmError;
use crate::git::types::MergeResult;
use crate::git::{GitBranches, GitMutations, GitWorktrees};
use crate::repo::RepoContext;
use crate::state;

/// Result of a fetch-and-fast-forward operation.
pub enum FetchResult {
    Updated { old_ref: String, new_ref: String },
    UpToDate,
    Diverged,
    NoUpstream,
}

/// Fetch from origin and fast-forward the base branch.
///
/// Uses `worktree_list()` to find the correct worktree when the base branch is
/// checked out (which may be a linked worktree after swap). Falls back to
/// `force_branch` when the base branch is stashed (not checked out anywhere).
pub fn fetch_and_ff(
    ctx: &RepoContext,
    git: &(impl GitBranches + GitWorktrees + GitMutations),
) -> Result<FetchResult, WkmError> {
    let wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;
    let base = &wkm_state.config.base_branch;

    if git.remote_tracking_branch(base)?.is_none() {
        return Ok(FetchResult::NoUpstream);
    }

    let old_ref = git.branch_ref(base)?;

    git.fetch("origin")?;

    let remote_ref = format!("origin/{base}");

    // Find which worktree has the base branch checked out
    let worktrees = git.worktree_list()?;
    let base_wt = worktrees
        .iter()
        .find(|wt| wt.branch.as_deref() == Some(base.as_str()));

    if let Some(wt) = base_wt {
        // Branch is checked out — merge in that worktree
        match git.merge_ff_only(&wt.path, &remote_ref)? {
            MergeResult::Clean => {}
            MergeResult::UpToDate => return Ok(FetchResult::UpToDate),
            MergeResult::Conflict { .. } | MergeResult::NotFastForward => {
                return Ok(FetchResult::Diverged);
            }
        }
    } else {
        // Branch not checked out — update ref directly
        if !git.is_ancestor(base, &remote_ref)? {
            return Ok(FetchResult::Diverged);
        }
        let new_commit = git.branch_ref(&remote_ref)?;
        if new_commit == old_ref {
            return Ok(FetchResult::UpToDate);
        }
        git.force_branch(base, &new_commit)?;
    }

    let new_ref = git.branch_ref(base)?;
    Ok(FetchResult::Updated { old_ref, new_ref })
}
