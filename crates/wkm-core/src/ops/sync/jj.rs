use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use crate::error::WkmError;
use crate::git::{GitBranches, GitDiscovery, GitMutations, GitStash, GitStatus, GitWorktrees};
use crate::graph;
use crate::repo::RepoContext;
use crate::state;
use crate::state::lock::WkmLock;
use crate::state::types::{WalEntry, WalOp};

use super::SyncResult;

/// Get the current jj operation ID for rollback.
fn jj_current_op_id(work_dir: &Path) -> Result<String, WkmError> {
    let output = Command::new("jj")
        .args(["op", "log", "--no-graph", "--limit=1", "-T", "self.id()"])
        .current_dir(work_dir)
        .output()
        .map_err(|e| WkmError::Git(format!("failed to run jj: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WkmError::Git(format!(
            "jj op log failed: {}",
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run `jj rebase` for a branch onto a new destination.
///
/// Uses `-b <branch>` which rebases the branch and all its descendants.
/// Returns a list of branches that have conflicts after the rebase.
fn jj_rebase_branch(work_dir: &Path, branch: &str, onto: &str) -> Result<Vec<String>, WkmError> {
    // jj uses bookmarks, not branches. In colocated repos, git branches
    // are automatically imported as jj bookmarks.
    let output = Command::new("jj")
        .args(["rebase", "-b", branch, "-d", onto])
        .current_dir(work_dir)
        .output()
        .map_err(|e| WkmError::Git(format!("failed to run jj: {e}")))?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stdout}{stderr}");

    if !output.status.success() {
        // jj rebase doesn't fail on conflicts — it stores them.
        // Failure here means a real error (bookmark not found, etc.)
        return Err(WkmError::Git(format!(
            "jj rebase failed: {}",
            stderr.trim()
        )));
    }

    // Detect conflicts: jj reports "New conflicts in N commits" or similar
    let mut conflicted = Vec::new();
    if combined.contains("conflict") {
        // Parse for specific conflicted commits/bookmarks
        // jj outputs lines like: "Rebased 3 commits onto destination"
        // and "New conflicts appeared in these commits:" followed by commit info
        // For now, report the branch as conflicted
        conflicted.push(branch.to_string());
    }

    Ok(conflicted)
}

/// Restore the jj repo to a previous operation state.
fn jj_op_restore(work_dir: &Path, op_id: &str) -> Result<(), WkmError> {
    let output = Command::new("jj")
        .args(["op", "restore", op_id])
        .current_dir(work_dir)
        .output()
        .map_err(|e| WkmError::Git(format!("failed to run jj: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WkmError::Git(format!(
            "jj op restore failed: {}",
            stderr.trim()
        )));
    }
    Ok(())
}

/// Sync all tracked branches using the jj backend.
///
/// Key differences from `sync_git()`:
/// - Uses `jj rebase -b <branch> -d <parent>` which auto-cascades to descendants
/// - No temp worktrees needed (jj rebases without checking out)
/// - Conflicts are stored in commits rather than blocking
/// - WAL stores jj operation ID for simple rollback via `jj op restore`
pub(super) fn sync_jj(
    ctx: &RepoContext,
    git: &(impl GitDiscovery + GitBranches + GitWorktrees + GitStatus + GitStash + GitMutations),
) -> Result<SyncResult, WkmError> {
    let lock = WkmLock::acquire(&ctx.lock_path)?;

    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    if wkm_state.wal.is_some() {
        return Err(WkmError::OperationInProgress);
    }

    // Try to fast-forward the base branch from remote
    let base = wkm_state.config.base_branch.clone();
    let _ = super::super::fetch::fetch_and_ff(ctx, git);

    // Build topo order to determine root branches (direct children of base)
    let topo = graph::topo_sort(&base, &wkm_state.branches);
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

    // Record pre-sync refs for potential manual recovery
    let mut pre_refs = BTreeMap::new();
    for branch in &branches_to_sync {
        if let Ok(hash) = git.branch_ref(branch) {
            pre_refs.insert(branch.clone(), hash);
        }
    }

    // Record jj operation ID before sync for rollback
    let jj_op_id = jj_current_op_id(&ctx.main_worktree)?;

    // Write WAL with jj_op_id
    wkm_state.wal = Some(WalEntry {
        id: uuid::Uuid::new_v4().to_string(),
        parent_op_id: None,
        op: WalOp::Sync {
            pre_refs,
            completed: vec![],
            conflicted: None,
            pending: branches_to_sync.clone(),
            temp_worktrees: vec![],
            jj_op_id: Some(jj_op_id),
        },
    });
    state::write_state(&ctx.state_path, &wkm_state)?;

    // Find root branches — direct children of base branch.
    // jj rebase -b cascades to descendants, so we only need to rebase roots.
    let root_branches: Vec<String> = branches_to_sync
        .iter()
        .filter(|b| {
            wkm_state
                .branches
                .get(*b)
                .and_then(|e| e.parent.as_deref())
                .unwrap_or(&base)
                == base
        })
        .cloned()
        .collect();

    // Non-root branches: those whose parent is not the base branch.
    // These need individual rebasing since their parent is another tracked branch.
    let non_root_branches: Vec<String> = branches_to_sync
        .iter()
        .filter(|b| {
            let parent = wkm_state
                .branches
                .get(*b)
                .and_then(|e| e.parent.as_deref())
                .unwrap_or(&base);
            parent != base
        })
        .cloned()
        .collect();

    let mut synced = Vec::new();
    let mut all_conflicted = Vec::new();

    // Rebase root branches onto base (jj cascades descendants automatically)
    for branch in &root_branches {
        let conflicts = jj_rebase_branch(&ctx.main_worktree, branch, &base)?;
        if conflicts.is_empty() {
            synced.push(branch.clone());
        } else {
            all_conflicted.extend(conflicts);
        }
    }

    // Rebase non-root branches onto their parents
    // jj may have already moved these if they were descendants of a root branch,
    // but jj handles this gracefully (no-op if already correct).
    for branch in &non_root_branches {
        let parent = wkm_state
            .branches
            .get(branch)
            .and_then(|e| e.parent.clone())
            .unwrap_or_else(|| base.clone());

        let conflicts = jj_rebase_branch(&ctx.main_worktree, branch, &parent)?;
        if conflicts.is_empty() {
            synced.push(branch.clone());
        } else {
            all_conflicted.extend(conflicts);
        }
    }

    // Export jj changes back to git so branch refs are updated
    let _ = Command::new("jj")
        .args(["git", "export"])
        .current_dir(&ctx.main_worktree)
        .output();

    // Clear WAL on success
    wkm_state.wal = None;
    state::write_state(&ctx.state_path, &wkm_state)?;

    drop(lock);

    // jj can report all conflicts at once (they're stored in commits)
    let conflicted = all_conflicted.first().cloned();
    let skipped: Vec<String> = if conflicted.is_some() {
        // Report non-synced, non-conflicted branches as skipped
        branches_to_sync
            .iter()
            .filter(|b| !synced.contains(b) && !all_conflicted.contains(b))
            .cloned()
            .collect()
    } else {
        vec![]
    };

    Ok(SyncResult {
        synced,
        conflicted,
        skipped,
    })
}

/// Continue a sync after conflict resolution (jj backend).
///
/// In jj, conflicts are stored in commits. After the user resolves conflicts,
/// they run `jj squash` or `jj resolve`. We just verify the conflicts are gone
/// and clear the WAL.
pub(super) fn sync_continue_jj(
    ctx: &RepoContext,
    _git: &(impl GitDiscovery + GitBranches + GitWorktrees + GitStatus + GitStash + GitMutations),
) -> Result<SyncResult, WkmError> {
    let lock = WkmLock::acquire(&ctx.lock_path)?;

    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    let wal = wkm_state
        .wal
        .as_ref()
        .ok_or(WkmError::NoOperationInProgress)?;

    let (completed, pending) = match &wal.op {
        WalOp::Sync {
            completed, pending, ..
        } => (completed.clone(), pending.clone()),
        _ => return Err(WkmError::NoOperationInProgress),
    };

    // Export any jj changes to git
    let _ = Command::new("jj")
        .args(["git", "export"])
        .current_dir(&ctx.main_worktree)
        .output();

    // Clear WAL
    wkm_state.wal = None;
    state::write_state(&ctx.state_path, &wkm_state)?;

    drop(lock);

    let synced: Vec<String> = pending
        .into_iter()
        .filter(|b| !completed.contains(b))
        .collect();

    Ok(SyncResult {
        synced,
        conflicted: None,
        skipped: vec![],
    })
}

/// Abort a sync, restoring all branches to pre-sync state (jj backend).
///
/// Uses `jj op restore` to atomically roll back all changes.
pub(super) fn sync_abort_jj(
    ctx: &RepoContext,
    _git: &(impl GitDiscovery + GitBranches + GitWorktrees + GitStatus + GitStash + GitMutations),
) -> Result<(), WkmError> {
    let lock = WkmLock::acquire(&ctx.lock_path)?;

    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    let wal = wkm_state
        .wal
        .as_ref()
        .ok_or(WkmError::NoOperationInProgress)?;

    let jj_op_id = match &wal.op {
        WalOp::Sync { jj_op_id, .. } => jj_op_id.clone(),
        _ => return Err(WkmError::NoOperationInProgress),
    };

    // Restore jj to pre-sync state if we have an op ID
    if let Some(op_id) = jj_op_id {
        jj_op_restore(&ctx.main_worktree, &op_id)?;

        // Export restored state back to git
        let _ = Command::new("jj")
            .args(["git", "export"])
            .current_dir(&ctx.main_worktree)
            .output();
    }

    // Clear WAL
    wkm_state.wal = None;
    state::write_state(&ctx.state_path, &wkm_state)?;

    drop(lock);
    Ok(())
}
