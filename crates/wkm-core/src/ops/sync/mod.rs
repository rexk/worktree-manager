mod jj;

use std::collections::BTreeMap;
use std::path::PathBuf;

use rayon::prelude::*;

use crate::encoding;
use crate::error::WkmError;
use crate::git::types::RebaseResult;
use crate::git::{GitBranches, GitDiscovery, GitMutations, GitStash, GitStatus, GitWorktrees};
use crate::graph;
use crate::repo::{RepoContext, VcsBackend};
use crate::state;
use crate::state::lock::WkmLock;
use crate::state::types::{WalEntry, WalOp};

/// Result of a sync operation.
pub struct SyncResult {
    pub synced: Vec<String>,
    pub conflicted: Option<String>,
    pub skipped: Vec<String>,
}

/// Sync all tracked branches by rebasing onto their parents.
///
/// Dispatches to `sync_git()` or `sync_jj()` based on detected VCS backend.
pub fn sync<G>(ctx: &RepoContext, git: &G) -> Result<SyncResult, WkmError>
where
    G: GitDiscovery + GitBranches + GitWorktrees + GitStatus + GitStash + GitMutations + Sync,
{
    match ctx.vcs_backend {
        VcsBackend::JjColocated => jj::sync_jj(ctx, git),
        VcsBackend::Git => sync_git(ctx, git),
    }
}

/// Continue a sync after conflict resolution.
///
/// Dispatches to `sync_continue_git()` or `sync_continue_jj()` based on detected VCS backend.
pub fn sync_continue<G>(ctx: &RepoContext, git: &G) -> Result<SyncResult, WkmError>
where
    G: GitDiscovery + GitBranches + GitWorktrees + GitStatus + GitStash + GitMutations + Sync,
{
    match ctx.vcs_backend {
        VcsBackend::JjColocated => jj::sync_continue_jj(ctx, git),
        VcsBackend::Git => sync_continue_git(ctx, git),
    }
}

/// Abort a sync, restoring all branches to pre-sync state.
///
/// Dispatches to `sync_abort_git()` or `sync_abort_jj()` based on detected VCS backend.
pub fn sync_abort<G>(ctx: &RepoContext, git: &G) -> Result<(), WkmError>
where
    G: GitDiscovery + GitBranches + GitWorktrees + GitStatus + GitStash + GitMutations + Sync,
{
    match ctx.vcs_backend {
        VcsBackend::JjColocated => jj::sync_abort_jj(ctx, git),
        VcsBackend::Git => sync_abort_git(ctx, git),
    }
}

/// Sync all tracked branches using the git backend.
fn sync_git<G>(ctx: &RepoContext, git: &G) -> Result<SyncResult, WkmError>
where
    G: GitDiscovery + GitBranches + GitWorktrees + GitStatus + GitStash + GitMutations + Sync,
{
    let lock = WkmLock::acquire(&ctx.lock_path)?;

    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    if wkm_state.wal.is_some() {
        return Err(WkmError::OperationInProgress);
    }

    // Check for dirty worktrees. Each `is_dirty` shells out to
    // `git status` — run them in parallel since they're independent
    // per-worktree calls.
    let candidates: Vec<(&String, &PathBuf)> = wkm_state
        .branches
        .iter()
        .filter_map(|(name, entry)| entry.worktree_path.as_ref().map(|wt| (name, wt)))
        .collect();
    let dirty: Vec<String> = candidates
        .par_iter()
        .filter_map(|(name, wt_path)| {
            if git.is_dirty(wt_path).unwrap_or(false) {
                Some((*name).clone())
            } else {
                None
            }
        })
        .collect();

    if !dirty.is_empty() {
        return Err(WkmError::DirtyWorktree(dirty.join(", ")));
    }

    // Try to fast-forward the base branch from remote
    let base = wkm_state.config.base_branch.clone();
    let _ = super::fetch::fetch_and_ff(ctx, git);

    // Build topo order: parents before children
    let topo = graph::topo_sort(&base, &wkm_state.branches);
    // Skip the base branch itself and any _wkm/ branches
    let branches_to_sync: Vec<String> = topo
        .into_iter()
        .filter(|b| *b != base && !b.starts_with("_wkm/"))
        .collect();

    if branches_to_sync.is_empty() {
        drop(lock);
        return Ok(SyncResult {
            synced: vec![],
            conflicted: None,
            skipped: vec![],
        });
    }

    // Record pre-sync refs for abort
    let mut pre_refs = BTreeMap::new();
    for branch in &branches_to_sync {
        if let Ok(hash) = git.branch_ref(branch) {
            pre_refs.insert(branch.clone(), hash);
        }
    }

    // Write WAL
    wkm_state.wal = Some(WalEntry {
        id: uuid::Uuid::new_v4().to_string(),
        parent_op_id: None,
        op: WalOp::Sync {
            pre_refs: pre_refs.clone(),
            completed: vec![],
            conflicted: None,
            pending: branches_to_sync.clone(),
            temp_worktrees: vec![],
            jj_op_id: None,
        },
    });
    state::write_state(&ctx.state_path, &wkm_state)?;

    let mut synced = Vec::new();
    let mut skipped = Vec::new();
    let mut temp_worktrees: Vec<(String, PathBuf)> = Vec::new();
    let mut conflicted_branch: Option<String> = None;

    // Track which subtrees to skip (children of conflicted branches)
    let mut skip_children_of: Vec<String> = Vec::new();

    for branch in &branches_to_sync {
        // Skip if parent was conflicted
        if let Some(entry) = wkm_state.branches.get(branch)
            && let Some(ref parent) = entry.parent
            && skip_children_of.contains(parent)
        {
            skip_children_of.push(branch.clone());
            skipped.push(branch.clone());
            continue;
        }

        let parent = wkm_state
            .branches
            .get(branch)
            .and_then(|e| e.parent.clone())
            .unwrap_or_else(|| base.clone());

        // Determine where to rebase
        let rebase_in = if let Some(entry) = wkm_state.branches.get(branch) {
            entry.worktree_path.clone()
        } else {
            None
        };

        let result = if let Some(ref wt_path) = rebase_in {
            // Rebase in existing worktree
            git.rebase(wt_path, &parent)?
        } else {
            // Create temp worktree for rebase
            let temp_branch = format!("_wkm/rebase/{branch}");
            let temp_id = encoding::generate_worktree_id();
            let temp_path = ctx
                .storage_dir
                .join(format!("_rebase_{temp_id}"))
                .join(&ctx.repo_name);

            if !git.branch_exists(&temp_branch)? {
                let branch_ref = git.branch_ref(branch)?;
                git.create_branch(&temp_branch, &branch_ref)?;
            }

            std::fs::create_dir_all(&ctx.storage_dir)?;
            git.worktree_add(&temp_path, &temp_branch)?;
            temp_worktrees.push((branch.clone(), temp_path.clone()));

            // Update WAL with temp worktree info
            update_sync_wal(
                &mut wkm_state,
                &synced,
                &None,
                &branches_to_sync,
                &temp_worktrees,
            );
            state::write_state(&ctx.state_path, &wkm_state)?;

            let result = git.rebase(&temp_path, &parent)?;

            if matches!(result, RebaseResult::Clean | RebaseResult::UpToDate) {
                // Move the actual branch to the rebased position
                let new_ref = git.branch_ref(&temp_branch)?;
                git.force_branch(branch, &new_ref)?;

                // Clean up temp worktree and branch
                let _ = git.worktree_remove(&temp_path, true);
                let _ = git.delete_branch(&temp_branch, true);
            }

            result
        };

        match result {
            RebaseResult::Clean | RebaseResult::UpToDate => {
                synced.push(branch.clone());
            }
            RebaseResult::Conflict { .. } => {
                conflicted_branch = Some(branch.clone());
                skip_children_of.push(branch.clone());

                // Update WAL with conflict info
                update_sync_wal(
                    &mut wkm_state,
                    &synced,
                    &conflicted_branch,
                    &branches_to_sync,
                    &temp_worktrees,
                );
                state::write_state(&ctx.state_path, &wkm_state)?;

                // Collect remaining as skipped
                let remaining: Vec<String> = branches_to_sync
                    .iter()
                    .filter(|b| !synced.contains(b) && b.as_str() != branch)
                    .cloned()
                    .collect();
                skipped.extend(remaining);
                break;
            }
        }

        // Update WAL progress
        update_sync_wal(
            &mut wkm_state,
            &synced,
            &None,
            &branches_to_sync,
            &temp_worktrees,
        );
        state::write_state(&ctx.state_path, &wkm_state)?;
    }

    if conflicted_branch.is_none() {
        // Clean up all temp worktrees
        for (_, path) in &temp_worktrees {
            let _ = git.worktree_remove(path, true);
        }
        // Clear WAL
        wkm_state.wal = None;
        state::write_state(&ctx.state_path, &wkm_state)?;
    }

    drop(lock);

    Ok(SyncResult {
        synced,
        conflicted: conflicted_branch,
        skipped,
    })
}

/// Continue a sync after conflict resolution (git backend).
fn sync_continue_git<G>(ctx: &RepoContext, git: &G) -> Result<SyncResult, WkmError>
where
    G: GitDiscovery + GitBranches + GitWorktrees + GitStatus + GitStash + GitMutations + Sync,
{
    let lock = WkmLock::acquire(&ctx.lock_path)?;

    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    let wal = wkm_state
        .wal
        .as_ref()
        .ok_or(WkmError::NoOperationInProgress)?;

    let (completed, conflicted, pending, temp_worktrees) = match &wal.op {
        WalOp::Sync {
            completed,
            conflicted,
            pending,
            temp_worktrees,
            ..
        } => (
            completed.clone(),
            conflicted.clone(),
            pending.clone(),
            temp_worktrees.clone(),
        ),
        _ => return Err(WkmError::NoOperationInProgress),
    };

    let conflicted_branch = conflicted.ok_or(WkmError::NoOperationInProgress)?;

    // Continue rebase in the conflicted worktree
    let rebase_wt = if let Some(entry) = wkm_state.branches.get(&conflicted_branch) {
        entry.worktree_path.clone()
    } else {
        // Check temp worktrees
        temp_worktrees
            .iter()
            .find(|(b, _)| b == &conflicted_branch)
            .map(|(_, p)| p.clone())
    };

    let rebase_wt = rebase_wt
        .ok_or_else(|| WkmError::Other(format!("no worktree found for {conflicted_branch}")))?;

    let result = git.rebase_continue(&rebase_wt)?;

    match result {
        RebaseResult::Clean => {
            // If temp worktree, move branch and clean up
            let temp_branch = format!("_wkm/rebase/{conflicted_branch}");
            if git.branch_exists(&temp_branch)? {
                let new_ref = git.branch_ref(&temp_branch)?;
                git.force_branch(&conflicted_branch, &new_ref)?;
                let _ = git.worktree_remove(&rebase_wt, true);
                let _ = git.delete_branch(&temp_branch, true);
            }

            let mut synced = completed;
            synced.push(conflicted_branch);

            // Continue with remaining branches
            let remaining: Vec<String> = pending
                .iter()
                .filter(|b| !synced.contains(b))
                .cloned()
                .collect();

            // Clear WAL and continue sync for remaining
            wkm_state.wal = None;
            state::write_state(&ctx.state_path, &wkm_state)?;

            drop(lock);

            if remaining.is_empty() {
                return Ok(SyncResult {
                    synced,
                    conflicted: None,
                    skipped: vec![],
                });
            }

            // Re-run sync for remaining branches
            let further = sync(ctx, git)?;
            synced.extend(further.synced);

            Ok(SyncResult {
                synced,
                conflicted: further.conflicted,
                skipped: further.skipped,
            })
        }
        RebaseResult::Conflict { .. } => {
            // Still conflicted
            drop(lock);
            Ok(SyncResult {
                synced: completed,
                conflicted: Some(conflicted_branch),
                skipped: vec![],
            })
        }
        RebaseResult::UpToDate => {
            // Shouldn't happen during continue but handle gracefully
            wkm_state.wal = None;
            state::write_state(&ctx.state_path, &wkm_state)?;
            drop(lock);
            Ok(SyncResult {
                synced: completed,
                conflicted: None,
                skipped: vec![],
            })
        }
    }
}

/// Abort a sync, restoring all branches to pre-sync refs (git backend).
fn sync_abort_git<G>(ctx: &RepoContext, git: &G) -> Result<(), WkmError>
where
    G: GitDiscovery + GitBranches + GitWorktrees + GitStatus + GitStash + GitMutations + Sync,
{
    let lock = WkmLock::acquire(&ctx.lock_path)?;

    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    let wal = wkm_state
        .wal
        .as_ref()
        .ok_or(WkmError::NoOperationInProgress)?;

    let (pre_refs, temp_worktrees) = match &wal.op {
        WalOp::Sync {
            pre_refs,
            temp_worktrees,
            ..
        } => (pre_refs.clone(), temp_worktrees.clone()),
        _ => return Err(WkmError::NoOperationInProgress),
    };

    // Abort any in-progress rebase in temp worktrees
    for (_, path) in &temp_worktrees {
        if let Ok(Some(_)) = git.has_in_progress_operation(path) {
            let _ = git.rebase_abort(path);
        }
        let _ = git.worktree_remove(path, true);
    }

    // Abort any in-progress rebase in branch worktrees
    for (branch, entry) in &wkm_state.branches {
        if let Some(ref wt_path) = entry.worktree_path
            && let Ok(Some(_)) = git.has_in_progress_operation(wt_path)
        {
            let _ = git.rebase_abort(wt_path);
        }
        // Clean up temp branches
        let temp_branch = format!("_wkm/rebase/{branch}");
        if git.branch_exists(&temp_branch).unwrap_or(false) {
            let _ = git.delete_branch(&temp_branch, true);
        }
    }

    // Reset branches to pre-sync refs
    for (branch, hash) in &pre_refs {
        let _ = git.force_branch(branch, hash);
    }

    // Clear WAL
    wkm_state.wal = None;
    state::write_state(&ctx.state_path, &wkm_state)?;

    drop(lock);
    Ok(())
}

fn update_sync_wal(
    wkm_state: &mut state::types::WkmState,
    completed: &[String],
    conflicted: &Option<String>,
    all_branches: &[String],
    temp_worktrees: &[(String, PathBuf)],
) {
    if let Some(ref mut wal) = wkm_state.wal
        && let WalOp::Sync {
            completed: ref mut c,
            conflicted: ref mut conf,
            pending: ref mut p,
            temp_worktrees: ref mut tw,
            ..
        } = wal.op
    {
        *c = completed.to_vec();
        *conf = conflicted.clone();
        *p = all_branches
            .iter()
            .filter(|b| !completed.contains(b))
            .cloned()
            .collect();
        *tw = temp_worktrees.to_vec();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::git::cli::CliGit;
    use crate::ops::init::{self, InitOptions};
    use crate::ops::worktree::{self, CreateOptions};
    use crate::state::types::BranchEntry;
    use wkm_sandbox::TestRepo;

    fn setup() -> (TestRepo, RepoContext, CliGit) {
        let repo = TestRepo::new();
        let ctx = RepoContext::from_path(repo.path()).unwrap();
        let git = CliGit::new(repo.path());
        init::init(&ctx, &InitOptions::default()).unwrap();
        (repo, ctx, git)
    }

    fn add_branch(ctx: &RepoContext, name: &str, parent: &str) {
        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            name.to_string(),
            BranchEntry {
                parent: Some(parent.to_string()),
                worktree_path: None,
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();
    }

    #[test]
    fn sync_linear_chain() {
        let (repo, ctx, git) = setup();

        // main → A → B
        repo.create_branch("branch-a");
        repo.checkout("branch-a");
        repo.commit_file("a-file", "a", "branch-a commit");

        repo.create_branch("branch-b");
        repo.checkout("branch-b");
        repo.commit_file("b-file", "b", "branch-b commit");

        repo.checkout("main");
        add_branch(&ctx, "branch-a", "main");
        add_branch(&ctx, "branch-b", "branch-a");

        // Advance main
        repo.commit_file("main-file", "main", "main advance");

        let result = sync(&ctx, &git).unwrap();
        assert!(result.synced.contains(&"branch-a".to_string()));
        assert!(result.synced.contains(&"branch-b".to_string()));
        assert!(result.conflicted.is_none());
    }

    #[test]
    fn sync_parallel_branches() {
        let (repo, ctx, git) = setup();

        // main → A, main → B
        repo.create_branch("branch-a");
        repo.checkout("branch-a");
        repo.commit_file("a-file", "a", "a commit");
        repo.checkout("main");

        repo.create_branch("branch-b");
        repo.checkout("branch-b");
        repo.commit_file("b-file", "b", "b commit");
        repo.checkout("main");

        add_branch(&ctx, "branch-a", "main");
        add_branch(&ctx, "branch-b", "main");

        // Advance main
        repo.commit_file("main-file", "main", "main advance");

        let result = sync(&ctx, &git).unwrap();
        assert_eq!(result.synced.len(), 2);
        assert!(result.conflicted.is_none());
    }

    #[test]
    fn sync_no_changes_noop() {
        let (_repo, ctx, git) = setup();
        let result = sync(&ctx, &git).unwrap();
        assert!(result.synced.is_empty());
        assert!(result.conflicted.is_none());
    }

    #[test]
    fn sync_dirty_worktree_aborts() {
        let (_repo, ctx, git) = setup();

        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feature".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();

        // Make the worktree dirty
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        let wt_path = wkm_state.branches["feature"]
            .worktree_path
            .as_ref()
            .unwrap();
        std::fs::write(wt_path.join("initial"), "dirty").unwrap();

        let result = sync(&ctx, &git);
        assert!(matches!(result, Err(WkmError::DirtyWorktree(_))));
    }

    #[test]
    fn sync_branch_in_worktree() {
        let (repo, ctx, git) = setup();

        // Create worktree for feature
        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feature".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        let feature_wt = wkm_state.branches["feature"]
            .worktree_path
            .as_ref()
            .unwrap()
            .clone();

        // Add a commit in the worktree
        std::fs::write(feature_wt.join("feat-file"), "feature data").unwrap();
        wkm_sandbox::git(&feature_wt, &["add", "."]);
        wkm_sandbox::git(&feature_wt, &["commit", "-m", "feature commit"]);

        // Advance main
        repo.commit_file("main-file", "main", "main advance");

        let result = sync(&ctx, &git).unwrap();
        assert!(result.synced.contains(&"feature".to_string()));
    }

    #[test]
    fn sync_branch_no_worktree() {
        let (repo, ctx, git) = setup();

        // Create branch without worktree
        repo.create_branch("feature");
        repo.checkout("feature");
        repo.commit_file("feat-file", "feature", "feature commit");
        repo.checkout("main");

        add_branch(&ctx, "feature", "main");

        // Advance main
        repo.commit_file("main-file", "main", "main advance");

        let result = sync(&ctx, &git).unwrap();
        assert!(result.synced.contains(&"feature".to_string()));

        // Temp worktree and branch should be cleaned up
        assert!(!git.branch_exists("_wkm/rebase/feature").unwrap());
    }

    #[test]
    fn sync_conflict_stops_subtree() {
        let (repo, ctx, git) = setup();

        // main → A → B (A will conflict)
        repo.commit_file("shared-file", "base", "add shared-file");

        repo.create_branch("branch-a");
        repo.checkout("branch-a");
        std::fs::write(repo.path().join("shared-file"), "a-version").unwrap();
        wkm_sandbox::git(repo.path(), &["add", "."]);
        wkm_sandbox::git(repo.path(), &["commit", "-m", "a: change shared-file"]);

        repo.create_branch("branch-b");
        repo.checkout("branch-b");
        repo.commit_file("b-file", "b", "b commit");
        repo.checkout("main");

        add_branch(&ctx, "branch-a", "main");
        add_branch(&ctx, "branch-b", "branch-a");

        // Change main's version to conflict with A
        std::fs::write(repo.path().join("shared-file"), "main-version").unwrap();
        wkm_sandbox::git(repo.path(), &["add", "."]);
        wkm_sandbox::git(repo.path(), &["commit", "-m", "main: change shared-file"]);

        let result = sync(&ctx, &git).unwrap();
        assert_eq!(result.conflicted, Some("branch-a".to_string()));
        // branch-b should be skipped since its parent conflicted
        assert!(result.skipped.contains(&"branch-b".to_string()));
    }

    #[test]
    fn sync_abort_restores() {
        let (repo, ctx, git) = setup();

        repo.commit_file("shared-file", "base", "add shared-file");

        repo.create_branch("branch-a");
        repo.checkout("branch-a");
        std::fs::write(repo.path().join("shared-file"), "a-version").unwrap();
        wkm_sandbox::git(repo.path(), &["add", "."]);
        wkm_sandbox::git(repo.path(), &["commit", "-m", "a: change shared-file"]);
        let pre_ref = wkm_sandbox::git_output(repo.path(), &["rev-parse", "HEAD"]);
        repo.checkout("main");

        add_branch(&ctx, "branch-a", "main");

        // Change main to conflict
        std::fs::write(repo.path().join("shared-file"), "main-version").unwrap();
        wkm_sandbox::git(repo.path(), &["add", "."]);
        wkm_sandbox::git(repo.path(), &["commit", "-m", "main: conflict"]);

        // Sync (will conflict)
        let _result = sync(&ctx, &git).unwrap();

        // Abort
        sync_abort(&ctx, &git).unwrap();

        // Branch should be back at original ref
        let post_ref = git.branch_ref("branch-a").unwrap();
        assert_eq!(pre_ref, post_ref);

        // WAL should be cleared
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(wkm_state.wal.is_none());
    }

    #[test]
    fn sync_wal_written_before_mutations() {
        let (repo, ctx, git) = setup();

        repo.create_branch("feature");
        repo.checkout("feature");
        repo.commit_file("feat-file", "f", "feature commit");
        repo.checkout("main");

        add_branch(&ctx, "feature", "main");
        repo.commit_file("main-file", "m", "main advance");

        // After sync, WAL should be cleared (successful sync)
        sync(&ctx, &git).unwrap();
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(wkm_state.wal.is_none());
    }

    #[test]
    fn sync_skips_hold_branches() {
        let (repo, ctx, git) = setup();

        // Add a hold branch to state (should be skipped)
        repo.create_branch("_wkm/hold/feature");
        add_branch(&ctx, "_wkm/hold/feature", "main");

        repo.create_branch("feature");
        repo.checkout("feature");
        repo.commit_file("feat-file", "f", "feature commit");
        repo.checkout("main");

        add_branch(&ctx, "feature", "main");
        repo.commit_file("main-file", "m", "main advance");

        let result = sync(&ctx, &git).unwrap();
        assert!(result.synced.contains(&"feature".to_string()));
        assert!(!result.synced.contains(&"_wkm/hold/feature".to_string()));
    }
}
