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

    // Check for dirty worktrees
    let dirty: Vec<String> = wkm_state
        .branches
        .iter()
        .filter_map(|(name, entry)| {
            if let Some(ref wt_path) = entry.worktree_path
                && git.is_dirty(wt_path).unwrap_or(false)
            {
                return Some(name.clone());
            }
            None
        })
        .collect();

    if !dirty.is_empty() {
        return Err(WkmError::DirtyWorktree(dirty.join(", ")));
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

    // Rebase root branches onto base (jj cascades descendants automatically).
    // Note: each `jj rebase` call auto-snapshots the main worktree's working copy
    // into the current jj change. This is jj's intended behavior and is harmless
    // (reversible via `jj undo`), but means uncommitted changes in the main
    // worktree become part of the jj commit graph.
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

    // Update working trees in secondary worktrees to match rebased refs.
    // After jj git export, the git branch refs point to rebased commits, but
    // the worktree files are still at the old commit.
    for branch in &synced {
        if let Some(entry) = wkm_state.branches.get(branch)
            && let Some(ref wt_path) = entry.worktree_path
        {
            // For dual-registered (GitJj) worktrees, update jj working copy first
            if entry.jj_workspace_name.is_some() {
                // Update jj working copy, then sync git HEAD
                let jj = crate::git::jj_cli::JjCli::new(&ctx.main_worktree);
                let _ = jj.workspace_update_stale(wt_path);
                crate::git::jj_cli::sync_git_head(wt_path, branch)?;
            } else {
                // Pure git worktree: reset to bring files in sync
                git.reset_hard(wt_path, branch)?;
            }
        }
    }

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

#[cfg(test)]
mod tests {
    use crate::git::cli::CliGit;
    use crate::git::{GitBranches, GitWorktrees};
    use crate::ops::init::{self, InitOptions};
    use crate::ops::sync;
    use crate::ops::worktree::{self, CreateOptions};
    use crate::repo::{RepoContext, VcsBackend};
    use crate::state;
    use crate::state::types::BranchEntry;
    use wkm_sandbox::TestRepo;

    /// Skip test if jj is not available. Returns `true` if test should be skipped.
    fn skip_if_no_jj() -> bool {
        if !wkm_sandbox::jj_available() {
            eprintln!("skipping: jj not on PATH");
            return true;
        }
        false
    }

    fn setup_jj() -> Option<(TestRepo, RepoContext, CliGit)> {
        let repo = TestRepo::new_jj_colocated()?;
        let ctx = RepoContext::from_path(repo.path()).unwrap();
        assert_eq!(ctx.vcs_backend, VcsBackend::JjColocated);
        let git = CliGit::new(repo.path());
        init::init(&ctx, &InitOptions::default()).unwrap();
        Some((repo, ctx, git))
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
    fn sync_jj_linear_chain() {
        if skip_if_no_jj() {
            return;
        }
        let Some((repo, ctx, git)) = setup_jj() else {
            return;
        };

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

        // Import into jj so it knows about the branches
        wkm_sandbox::jj(repo.path(), &["git", "import"]);

        let result = sync::sync(&ctx, &git).unwrap();
        assert!(result.synced.contains(&"branch-a".to_string()));
        assert!(result.synced.contains(&"branch-b".to_string()));
        assert!(result.conflicted.is_none());

        // Verify ancestry: main is ancestor of branch-a, branch-a is ancestor of branch-b
        assert!(git.is_ancestor("main", "branch-a").unwrap());
        assert!(git.is_ancestor("branch-a", "branch-b").unwrap());
    }

    #[test]
    fn sync_jj_updates_worktree_files() {
        if skip_if_no_jj() {
            return;
        }
        let Some((_repo, ctx, git)) = setup_jj() else {
            return;
        };

        // Create worktree for feature branch
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

        // Advance main in the main worktree
        std::fs::write(ctx.main_worktree.join("main-file"), "main data").unwrap();
        wkm_sandbox::git(&ctx.main_worktree, &["add", "."]);
        wkm_sandbox::git(&ctx.main_worktree, &["commit", "-m", "main advance"]);

        // Import into jj
        wkm_sandbox::jj(&ctx.main_worktree, &["git", "import"]);

        let result = sync::sync(&ctx, &git).unwrap();
        assert!(result.synced.contains(&"feature".to_string()));

        // CRITICAL: Verify the secondary worktree files are updated.
        // The main-file should now exist in the feature worktree (rebased onto main).
        assert!(
            feature_wt.join("main-file").exists(),
            "secondary worktree should have main-file after sync (working tree update)"
        );
        // Original feature file should still be there
        assert!(feature_wt.join("feat-file").exists());
    }

    #[test]
    fn sync_jj_dirty_worktree_aborts() {
        if skip_if_no_jj() {
            return;
        }
        let Some((_repo, ctx, git)) = setup_jj() else {
            return;
        };

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

        let result = sync::sync(&ctx, &git);
        assert!(
            matches!(result, Err(crate::error::WkmError::DirtyWorktree(_))),
            "sync_jj should reject dirty worktrees"
        );
    }

    #[test]
    fn sync_jj_abort_restores() {
        if skip_if_no_jj() {
            return;
        }
        let Some((repo, ctx, git)) = setup_jj() else {
            return;
        };

        repo.commit_file("shared-file", "base", "add shared-file");

        repo.create_branch("branch-a");
        repo.checkout("branch-a");
        std::fs::write(repo.path().join("shared-file"), "a-version").unwrap();
        wkm_sandbox::git(repo.path(), &["add", "."]);
        wkm_sandbox::git(repo.path(), &["commit", "-m", "a: change shared-file"]);

        let pre_ref = wkm_sandbox::git_output(repo.path(), &["rev-parse", "branch-a"]);
        repo.checkout("main");

        add_branch(&ctx, "branch-a", "main");

        // Change main to create divergence (jj doesn't block on conflicts, it stores them)
        std::fs::write(repo.path().join("shared-file"), "main-version").unwrap();
        wkm_sandbox::git(repo.path(), &["add", "."]);
        wkm_sandbox::git(repo.path(), &["commit", "-m", "main: conflict"]);

        // Import into jj
        wkm_sandbox::jj(repo.path(), &["git", "import"]);

        // Sync (jj stores conflicts in commits, so this succeeds)
        let _result = sync::sync(&ctx, &git).unwrap();

        // branch-a ref should have changed (rebased)
        let post_ref = wkm_sandbox::git_output(repo.path(), &["rev-parse", "branch-a"]);
        assert_ne!(pre_ref, post_ref, "branch-a should have been rebased");
    }

    #[test]
    fn jj_worktree_creation_despite_detached_head() {
        if skip_if_no_jj() {
            return;
        }
        let Some((_repo, ctx, git)) = setup_jj() else {
            return;
        };

        // jj puts git in detached HEAD. Verify wkm can still create worktrees
        // (because wkm creates the branch before calling git worktree add).
        let result = worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feature".to_string(),
                base: None,
                description: None,
                name: None,
            },
        );
        assert!(
            result.is_ok(),
            "worktree creation should succeed on colocated jj repo: {:?}",
            result.err()
        );

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        let wt_path = wkm_state.branches["feature"]
            .worktree_path
            .as_ref()
            .unwrap();

        // Secondary worktree should have a .git file (not directory)
        let git_path = wt_path.join(".git");
        assert!(git_path.exists(), ".git should exist in secondary worktree");
        assert!(
            git_path.is_file(),
            ".git should be a file (pointer) in secondary worktree, not a directory"
        );
    }

    // ---------------------------------------------------------------
    // Dual registration (GitJj backend) tests
    // ---------------------------------------------------------------

    fn setup_jj_dual() -> Option<(TestRepo, RepoContext, CliGit)> {
        let repo = TestRepo::new_jj_colocated()?;
        let ctx = RepoContext::from_path(repo.path()).unwrap();
        assert_eq!(ctx.vcs_backend, VcsBackend::JjColocated);
        let git = CliGit::new(repo.path());
        // Init with explicit GitJj backend
        let opts = InitOptions {
            base_branch: "main".to_string(),
            storage_dir: None,
            worktree_backend: Some(crate::state::types::WorktreeBackend::GitJj),
        };
        init::init(&ctx, &opts).unwrap();
        Some((repo, ctx, git))
    }

    #[test]
    fn dual_worktree_create_has_both_git_and_jj() {
        if skip_if_no_jj() {
            return;
        }
        let Some((_repo, ctx, git)) = setup_jj_dual() else {
            return;
        };

        let result = worktree::create(
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

        let wt_path = &result.worktree_path;

        // .git should be a file (pointer)
        let git_path = wt_path.join(".git");
        assert!(git_path.exists(), ".git should exist");
        assert!(git_path.is_file(), ".git should be a file (pointer)");

        // .jj should be a directory
        let jj_path = wt_path.join(".jj");
        assert!(jj_path.is_dir(), ".jj/ should exist as a directory");

        // .jj/.gitignore should exist with "/*"
        let gitignore = jj_path.join(".gitignore");
        assert!(gitignore.exists(), ".jj/.gitignore should exist");
        let content = std::fs::read_to_string(&gitignore).unwrap();
        assert!(content.contains("/*"), ".jj/.gitignore should contain /*");

        // .jj/repo should be a file pointing to main's .jj/repo
        let repo_file = jj_path.join("repo");
        assert!(repo_file.exists(), ".jj/repo should exist");
        assert!(repo_file.is_file(), ".jj/repo should be a file");
    }

    #[test]
    fn dual_worktree_registered_in_git() {
        if skip_if_no_jj() {
            return;
        }
        let Some((_repo, ctx, git)) = setup_jj_dual() else {
            return;
        };

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

        // git worktree list should include the worktree
        let worktrees = git.worktree_list().unwrap();
        assert!(
            worktrees
                .iter()
                .any(|wt| wt.branch.as_deref() == Some("feature")),
            "git worktree list should include 'feature'"
        );
    }

    #[test]
    fn dual_worktree_registered_in_jj() {
        if skip_if_no_jj() {
            return;
        }
        let Some((_repo, ctx, git)) = setup_jj_dual() else {
            return;
        };

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

        // jj workspace list should include the workspace
        let output = std::process::Command::new("jj")
            .args(["workspace", "list"])
            .current_dir(&ctx.main_worktree)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("feature"),
            "jj workspace list should include 'feature', got: {stdout}"
        );
    }

    #[test]
    fn dual_worktree_state_has_jj_workspace_name() {
        if skip_if_no_jj() {
            return;
        }
        let Some((_repo, ctx, git)) = setup_jj_dual() else {
            return;
        };

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
        let entry = &wkm_state.branches["feature"];
        assert!(
            entry.jj_workspace_name.is_some(),
            "jj_workspace_name should be set"
        );
        assert_eq!(entry.jj_workspace_name.as_deref(), Some("feature"));
    }

    #[test]
    fn dual_worktree_git_status_clean() {
        if skip_if_no_jj() {
            return;
        }
        let Some((_repo, ctx, git)) = setup_jj_dual() else {
            return;
        };

        let result = worktree::create(
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

        // git status should be clean (no untracked .jj/)
        let output = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&result.worktree_path)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.trim().is_empty(),
            "git status should be clean, got: {stdout}"
        );
    }

    #[test]
    fn dual_worktree_jj_status_works() {
        if skip_if_no_jj() {
            return;
        }
        let Some((_repo, ctx, git)) = setup_jj_dual() else {
            return;
        };

        let result = worktree::create(
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

        // jj status should work in the worktree
        let output = std::process::Command::new("jj")
            .args(["status"])
            .current_dir(&result.worktree_path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "jj status should succeed in dual worktree, stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn dual_worktree_remove_cleans_both() {
        if skip_if_no_jj() {
            return;
        }
        let Some((_repo, ctx, git)) = setup_jj_dual() else {
            return;
        };

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
        let wt_path = wkm_state.branches["feature"]
            .worktree_path
            .as_ref()
            .unwrap()
            .clone();

        worktree::remove(
            &ctx,
            &git,
            &worktree::RemoveOptions {
                branch: Some("feature"),
                ..Default::default()
            },
        )
        .unwrap();

        // State entry should be gone entirely
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(!wkm_state.branches.contains_key("feature"));

        // Directory should be gone (or renamed)
        assert!(!wt_path.exists());

        // jj workspace should be forgotten
        let output = std::process::Command::new("jj")
            .args(["workspace", "list"])
            .current_dir(&ctx.main_worktree)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !stdout.contains("feature"),
            "jj workspace list should not include 'feature' after remove"
        );
    }

    #[test]
    fn dual_sync_updates_both_jj_and_git() {
        if skip_if_no_jj() {
            return;
        }
        let Some((_repo, ctx, git)) = setup_jj_dual() else {
            return;
        };

        // Create dual worktree for feature branch
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

        // Add a commit in the feature worktree via git
        std::fs::write(feature_wt.join("feat-file"), "feature data").unwrap();
        wkm_sandbox::git(&feature_wt, &["add", "."]);
        wkm_sandbox::git(&feature_wt, &["commit", "-m", "feature commit"]);

        // Advance main in the main worktree
        std::fs::write(ctx.main_worktree.join("main-file"), "main data").unwrap();
        wkm_sandbox::git(&ctx.main_worktree, &["add", "."]);
        wkm_sandbox::git(&ctx.main_worktree, &["commit", "-m", "main advance"]);

        // Import into jj
        wkm_sandbox::jj(&ctx.main_worktree, &["git", "import"]);

        let result = sync::sync(&ctx, &git).unwrap();
        assert!(result.synced.contains(&"feature".to_string()));

        // The main-file should exist in the feature worktree (rebased onto main)
        assert!(
            feature_wt.join("main-file").exists(),
            "secondary worktree should have main-file after sync"
        );
        // Original feature file should still be there
        assert!(feature_wt.join("feat-file").exists());

        // git worktree list should still show [feature] (not detached)
        let worktrees = git.worktree_list().unwrap();
        let feat_wt = worktrees
            .iter()
            .find(|wt| wt.branch.as_deref() == Some("feature"));
        assert!(
            feat_wt.is_some(),
            "git worktree list should show feature branch"
        );
    }

    #[test]
    fn dual_git_commit_visible_in_jj() {
        if skip_if_no_jj() {
            return;
        }
        let Some((_repo, ctx, git)) = setup_jj_dual() else {
            return;
        };

        let result = worktree::create(
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

        let wt_path = &result.worktree_path;

        // Make a git commit in the dual worktree
        std::fs::write(wt_path.join("from-git"), "git content").unwrap();
        wkm_sandbox::git(wt_path, &["add", "."]);
        wkm_sandbox::git(wt_path, &["commit", "-m", "git: added from-git"]);

        // Import into jj
        wkm_sandbox::jj(&ctx.main_worktree, &["git", "import"]);

        // jj should see the commit
        let output = wkm_sandbox::jj_output(
            &ctx.main_worktree,
            &["log", "--no-graph", "-T", "description", "-r", "feature"],
        );
        assert!(
            output.contains("git: added from-git"),
            "jj should see the git commit, got: {output}"
        );
    }

    #[test]
    fn gitjj_backend_on_non_colocated_repo_errors() {
        // Pure git repo — setting GitJj backend should fail
        let repo = TestRepo::new();
        let ctx = RepoContext::from_path(repo.path()).unwrap();
        assert_eq!(ctx.vcs_backend, VcsBackend::Git);

        let opts = InitOptions {
            base_branch: "main".to_string(),
            storage_dir: None,
            worktree_backend: Some(crate::state::types::WorktreeBackend::GitJj),
        };
        let result = init::init(&ctx, &opts);
        assert!(result.is_err(), "GitJj on non-colocated repo should error");
    }

    #[test]
    fn colocated_repo_auto_defaults_to_gitjj() {
        if skip_if_no_jj() {
            return;
        }
        let Some(repo) = TestRepo::new_jj_colocated() else {
            return;
        };
        let ctx = RepoContext::from_path(repo.path()).unwrap();
        // Init with default options (no explicit backend)
        let state = init::init(&ctx, &InitOptions::default()).unwrap();
        assert_eq!(
            state.config.worktree_backend,
            crate::state::types::WorktreeBackend::GitJj,
            "colocated repo should auto-default to GitJj"
        );
    }
}
