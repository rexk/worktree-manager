use crate::error::WkmError;
use crate::git::{GitBranches, GitDiscovery, GitMutations, GitStash, GitStatus, GitWorktrees};
use crate::repo::RepoContext;
use crate::state;
use crate::state::lock::WkmLock;

/// Summary of what repair did.
#[derive(Debug, Default)]
pub struct RepairResult {
    pub wal_cleared: bool,
    pub stale_lock_removed: bool,
    pub git_worktree_repaired: bool,
    pub branches_removed: Vec<String>,
    pub branches_pruned: Vec<String>,
    pub worktree_paths_cleared: Vec<String>,
    pub worktree_paths_updated: Vec<String>,
    pub branches_adopted: Vec<String>,
    pub orphan_branches_deleted: Vec<String>,
    pub pending_removals_cleaned: Vec<String>,
    pub workspace_aliases_cleared: Vec<String>,
}

/// Run repair: enforce all invariants.
///
/// 1. Remove stale lockfile (dead PID)
/// 2. Run `git worktree repair` and `git worktree prune`
/// 3. Clear WAL (rollback incomplete ops)
/// 4. Remove state entries for branches that no longer exist in git
/// 5. Clear worktree_path for entries where the path no longer exists on disk
/// 6. Reconcile worktree_path for tracked branches against actual git worktrees
/// 7. Prune worktree-less state entries (unless they hold a stash or are the
///    branch currently hosted in the main worktree)
/// 8. Auto-adopt untracked branches checked out in worktrees
/// 9. Delete orphaned `_wkm/*` branches not referenced by state or WAL
/// 10. Clean up pending `.wkm-removing` directories
pub fn repair(
    ctx: &RepoContext,
    git: &(impl GitDiscovery + GitBranches + GitWorktrees + GitStatus + GitStash + GitMutations),
) -> Result<RepairResult, WkmError> {
    let mut result = RepairResult::default();

    // 1. Remove stale lockfile
    if WkmLock::is_stale(&ctx.lock_path)? {
        WkmLock::remove_stale(&ctx.lock_path)?;
        result.stale_lock_removed = true;
    }

    let lock = WkmLock::acquire(&ctx.lock_path)?;

    // 2. Run git worktree repair and prune to fix git-level metadata
    let repair_ok = git.worktree_repair().is_ok();
    let prune_ok = git.worktree_prune().is_ok();
    result.git_worktree_repaired = repair_ok || prune_ok;

    let mut wkm_state = match state::read_state(&ctx.state_path)? {
        Some(s) => s,
        None => return Err(WkmError::NotInitialized),
    };

    // 2. Clear WAL
    if wkm_state.wal.is_some() {
        // Simple rollback: just clear the WAL. For a full implementation,
        // we'd inspect the WAL op and undo partial work. For now, clearing
        // the WAL is the minimal safe action (user may need manual cleanup).
        wkm_state.wal = None;
        result.wal_cleared = true;
    }

    // Enumerate all local branches in a single git call. Used below for
    // existence checks and the `_wkm/*` orphan sweep (replaces N+1
    // `git rev-parse --verify` subprocesses and a separate `git branch`).
    let git_refs = git.branch_refs()?;

    // 3. Remove state entries for branches that no longer exist in git
    let branches_to_check: Vec<String> = wkm_state.branches.keys().cloned().collect();
    for branch in &branches_to_check {
        if !git_refs.contains_key(branch) {
            wkm_state.branches.remove(branch);
            result.branches_removed.push(branch.clone());
        }
    }

    // 4. Clear worktree_path for entries where the path no longer exists
    for (name, entry) in wkm_state.branches.iter_mut() {
        if let Some(ref wt_path) = entry.worktree_path
            && !wt_path.exists()
        {
            entry.worktree_path = None;
            result.worktree_paths_cleared.push(name.clone());
        }
    }

    // 5. Reconcile worktree_path for tracked branches against git worktree list.
    // The main worktree is not a wkm-managed worktree: branches checked out there
    // must have worktree_path = None (main-worktree membership is inferred at runtime).
    let worktrees = git.worktree_list()?;
    for (name, entry) in wkm_state.branches.iter_mut() {
        let actual_wt = worktrees
            .iter()
            .find(|wt| wt.branch.as_deref() == Some(name.as_str()))
            .filter(|wt| wt.path != ctx.main_worktree);
        match (actual_wt, &entry.worktree_path) {
            // Branch is in a secondary worktree but state doesn't know about it
            (Some(wt), None) => {
                entry.worktree_path = Some(wt.path.clone());
                result.worktree_paths_updated.push(name.clone());
            }
            // Branch is in a different secondary worktree than state thinks
            (Some(wt), Some(existing)) if *existing != wt.path => {
                entry.worktree_path = Some(wt.path.clone());
                result.worktree_paths_updated.push(name.clone());
            }
            // Branch is NOT in any secondary worktree but state has a path —
            // stale entry (often `Some(main_worktree)` left over from a past
            // check-out, or a path that happens to still exist on disk).
            (None, Some(_)) => {
                entry.worktree_path = None;
                result.worktree_paths_cleared.push(name.clone());
            }
            // State matches reality — nothing to do
            _ => {}
        }
    }

    // 6. Prune worktree-less state entries.
    //
    // After reconciliation, `worktree_path = None` means "not hosted in any
    // worktree". Such entries are legacy clutter (e.g. left over from the
    // pre-redesign `wkm worktree remove` behavior or from externally-deleted
    // worktrees). Drop them so `wkm list` stays in sync with reality.
    //
    // Safety:
    // - `stash_commit.is_some()` — entry holds a recoverable auto-stash;
    //   pruning would lose the reference (per SPEC §8.1, stash recovery
    //   reads `BranchEntry.stash_commit`).
    // - name matches `git.current_branch(main_worktree)` — the branch
    //   currently hosted in the main worktree has `worktree_path = None` by
    //   design (SPEC §5.3 invariant); it must not be pruned.
    let current_main_branch = git.current_branch(&ctx.main_worktree)?;
    let prune_candidates: Vec<String> = wkm_state
        .branches
        .iter()
        .filter(|(name, entry)| {
            entry.worktree_path.is_none()
                && entry.stash_commit.is_none()
                && current_main_branch.as_deref() != Some(name.as_str())
        })
        .map(|(name, _)| name.clone())
        .collect();
    for name in prune_candidates {
        wkm_state.branches.remove(&name);
        result.branches_pruned.push(name);
    }

    // 7. Auto-adopt untracked branches that are checked out in worktrees
    let base_branch = &wkm_state.config.base_branch.clone();
    let now = chrono::Utc::now().to_rfc3339();
    for wt in &worktrees {
        if let Some(ref branch) = wt.branch {
            // Skip base branch, internal branches, and already-tracked branches
            if branch == base_branch
                || branch.starts_with("_wkm/")
                || wkm_state.branches.contains_key(branch.as_str())
            {
                continue;
            }
            // Only record a worktree_path for secondary worktrees — a branch
            // sitting in the main worktree is tracked without a stored path.
            let worktree_path = if wt.path == ctx.main_worktree {
                None
            } else {
                Some(wt.path.clone())
            };
            wkm_state.branches.insert(
                branch.clone(),
                crate::state::types::BranchEntry {
                    parent: Some(base_branch.clone()),
                    worktree_path,
                    stash_commit: None,
                    jj_workspace_name: None,
                    description: None,
                    created_at: now.clone(),
                    previous_branch: None,
                },
            );
            result.branches_adopted.push(branch.clone());
        }
    }

    // 8a. Drop workspace alias entries whose path no longer exists or whose
    // alias fails validation (e.g. hand-edited invalid name).
    let stale_aliases: Vec<String> = wkm_state
        .workspaces
        .iter()
        .filter(|(alias, entry)| {
            crate::encoding::validate_workspace_alias(alias).is_err()
                || !entry.worktree_path.exists()
        })
        .map(|(alias, _)| alias.clone())
        .collect();
    for alias in &stale_aliases {
        wkm_state.workspaces.remove(alias);
        result.workspace_aliases_cleared.push(alias.clone());
    }

    // Live set of `_wkm/parked/<alias>` branches that should be preserved.
    let live_parked: std::collections::BTreeSet<String> = wkm_state
        .workspaces
        .keys()
        .map(|a| format!("_wkm/parked/{a}"))
        .collect();

    // 9. Delete orphaned `_wkm/*` branches
    // Collect all _wkm/* branches that exist
    let mut orphan_candidates: Vec<String> = Vec::new();
    for wt in &worktrees {
        if let Some(ref branch) = wt.branch
            && branch.starts_with("_wkm/")
            && !live_parked.contains(branch)
        {
            orphan_candidates.push(branch.clone());
        }
    }

    // A _wkm/* branch is orphaned if there's no WAL referencing it
    // (WAL is already cleared at this point, so all are orphaned)
    for branch in &orphan_candidates {
        // Remove the worktree first, then delete the branch
        if let Some(wt) = worktrees
            .iter()
            .find(|w| w.branch.as_deref() == Some(branch))
        {
            let _ = git.worktree_remove(&wt.path, true);
        }
        let _ = git.delete_branch(branch, true);
        result.orphan_branches_deleted.push(branch.clone());
    }

    // Also check for _wkm/* branches not in any worktree
    // (They might exist as regular branches without worktrees)
    for branch in git_refs.keys() {
        if branch.starts_with("_wkm/")
            && !orphan_candidates.contains(branch)
            && !live_parked.contains(branch)
        {
            let _ = git.delete_branch(branch, true);
            result.orphan_branches_deleted.push(branch.clone());
        }
    }

    // 10. Clean up leftover .wkm-removing directories in storage_dir
    if ctx.storage_dir.exists()
        && let Ok(entries) = std::fs::read_dir(&ctx.storage_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir()
                && path.extension().is_some_and(|ext| ext == "wkm-removing")
                && std::fs::remove_dir_all(&path).is_ok()
            {
                result
                    .pending_removals_cleaned
                    .push(path.display().to_string());
            }
        }
    }

    state::write_state(&ctx.state_path, &wkm_state)?;
    drop(lock);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::cli::CliGit;
    use crate::ops::init::{self, InitOptions};
    use crate::ops::worktree::{self, CreateOptions};
    use crate::state::types::{BranchEntry, SwapStep, WalEntry, WalOp};
    use wkm_sandbox::TestRepo;

    fn setup() -> (TestRepo, RepoContext, CliGit) {
        let repo = TestRepo::new();
        let ctx = RepoContext::from_path(repo.path()).unwrap();
        let git = CliGit::new(repo.path());
        init::init(&ctx, &InitOptions::default()).unwrap();
        (repo, ctx, git)
    }

    #[test]
    fn repair_stale_state_entry() {
        let (repo, ctx, git) = setup();
        repo.create_branch("ephemeral");

        // Track it in state
        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "ephemeral".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: None,
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        // Delete branch outside wkm
        wkm_sandbox::git(repo.path(), &["branch", "-D", "ephemeral"]);

        let result = repair(&ctx, &git).unwrap();
        assert!(result.branches_removed.contains(&"ephemeral".to_string()));

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(!wkm_state.branches.contains_key("ephemeral"));
    }

    #[test]
    fn repair_worktree_path_mismatch() {
        let (_repo, ctx, git) = setup();

        // Create worktree normally
        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feat".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();

        // Manually set worktree_path to a nonexistent path
        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        let real_wt_path = wkm_state.branches["feat"].worktree_path.clone().unwrap();
        wkm_state.branches.get_mut("feat").unwrap().worktree_path =
            Some("/tmp/nonexistent-wt-path-12345".into());
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let result = repair(&ctx, &git).unwrap();
        // Step 4 clears the bogus path, then step 5 reconciles from git worktree list
        assert!(result.worktree_paths_cleared.contains(&"feat".to_string()));
        assert!(result.worktree_paths_updated.contains(&"feat".to_string()));

        // The worktree path should be restored to the real path
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert_eq!(wkm_state.branches["feat"].worktree_path, Some(real_wt_path));
    }

    #[test]
    fn repair_incomplete_swap_wal() {
        let (_repo, ctx, git) = setup();

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.wal = Some(WalEntry {
            id: "test-wal".to_string(),
            parent_op_id: None,
            op: WalOp::Swap {
                source_branch: "main".to_string(),
                target_branch: "feat".to_string(),
                source_worktree: "/tmp/a".into(),
                target_worktree: None,
                main_stash: Some("abc123".to_string()),
                wt_stash: None,
                step: SwapStep::StashedMain,
            },
        });
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let result = repair(&ctx, &git).unwrap();
        assert!(result.wal_cleared);

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(wkm_state.wal.is_none());
    }

    #[test]
    fn repair_stale_lock_removed() {
        let (_repo, ctx, git) = setup();

        // Write a stale lockfile
        std::fs::write(&ctx.lock_path, "99999999").unwrap();

        let result = repair(&ctx, &git).unwrap();
        assert!(result.stale_lock_removed);
        assert!(!ctx.lock_path.exists());
    }

    #[test]
    fn repair_orphan_wkm_hold_branch() {
        let (repo, ctx, git) = setup();

        // Create an orphan _wkm/hold/feat branch
        repo.create_branch("_wkm/hold/feat");

        // Create a worktree for it so worktree_list picks it up
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("hold-wt");
        wkm_sandbox::git(
            repo.path(),
            &[
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "_wkm/hold/feat",
            ],
        );

        let result = repair(&ctx, &git).unwrap();
        assert!(
            result
                .orphan_branches_deleted
                .contains(&"_wkm/hold/feat".to_string())
        );
    }

    #[test]
    fn repair_cleans_wkm_removing_dirs() {
        let (_repo, ctx, git) = setup();

        // Create a fake .wkm-removing directory in the storage dir
        std::fs::create_dir_all(&ctx.storage_dir).unwrap();
        let leftover = ctx.storage_dir.join("some-branch.wkm-removing");
        std::fs::create_dir_all(&leftover).unwrap();
        std::fs::write(leftover.join("big-file"), "data").unwrap();
        assert!(leftover.exists());

        let result = repair(&ctx, &git).unwrap();
        assert!(!leftover.exists());
        assert_eq!(result.pending_removals_cleaned.len(), 1);
    }

    #[test]
    fn repair_orphan_wkm_branch_no_worktree() {
        let (repo, ctx, git) = setup();

        // Create a _wkm/ branch that has no worktree (e.g. leftover from a crash)
        repo.create_branch("_wkm/hold/stale");

        let result = repair(&ctx, &git).unwrap();
        assert!(
            result
                .orphan_branches_deleted
                .contains(&"_wkm/hold/stale".to_string()),
            "Expected orphan _wkm branch without worktree to be cleaned up"
        );

        // Verify the branch was actually deleted
        assert!(
            !git.branch_exists("_wkm/hold/stale").unwrap(),
            "Branch should be deleted after repair"
        );
    }

    #[test]
    fn repair_idempotent() {
        let (repo, ctx, git) = setup();
        repo.create_branch("ephemeral");

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "ephemeral".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: None,
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        // Delete branch outside wkm
        wkm_sandbox::git(repo.path(), &["branch", "-D", "ephemeral"]);

        let result1 = repair(&ctx, &git).unwrap();
        assert!(!result1.branches_removed.is_empty());

        // Second run should be a no-op
        let result2 = repair(&ctx, &git).unwrap();
        assert!(result2.branches_removed.is_empty());
        assert!(result2.branches_pruned.is_empty());
        assert!(!result2.wal_cleared);
        assert!(!result2.stale_lock_removed);
        assert!(result2.worktree_paths_cleared.is_empty());
        assert!(result2.orphan_branches_deleted.is_empty());
    }

    #[test]
    fn repair_adopts_untracked_worktree_branch() {
        let (repo, ctx, git) = setup();

        // Create a branch and worktree outside wkm
        repo.create_branch("external-feat");
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("ext-wt");
        wkm_sandbox::git(
            repo.path(),
            &[
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "external-feat",
            ],
        );

        // Branch is NOT in wkm state
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(!wkm_state.branches.contains_key("external-feat"));

        // Repair should auto-adopt it
        let result = repair(&ctx, &git).unwrap();
        assert!(
            result
                .branches_adopted
                .contains(&"external-feat".to_string()),
            "Expected repair to auto-adopt untracked worktree branch"
        );

        // Verify it's now in state with the correct worktree path
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(wkm_state.branches.contains_key("external-feat"));
        assert_eq!(
            wkm_state.branches["external-feat"].parent,
            Some("main".to_string())
        );
        assert!(wkm_state.branches["external-feat"].worktree_path.is_some());

        // Cleanup
        wkm_sandbox::git(
            repo.path(),
            &["worktree", "remove", wt_path.to_str().unwrap()],
        );
    }

    #[test]
    fn repair_reconciles_worktree_path_for_tracked_branch() {
        let (repo, ctx, git) = setup();

        // Create a branch tracked in wkm state but with no worktree_path
        repo.create_branch("tracked-feat");
        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "tracked-feat".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: None,
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        // Now create a worktree for it outside wkm
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("tracked-wt");
        wkm_sandbox::git(
            repo.path(),
            &["worktree", "add", wt_path.to_str().unwrap(), "tracked-feat"],
        );

        // Repair should update the worktree_path
        let result = repair(&ctx, &git).unwrap();
        assert!(
            result
                .worktree_paths_updated
                .contains(&"tracked-feat".to_string()),
            "Expected repair to update worktree path for tracked branch"
        );

        // Verify the path was updated
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(wkm_state.branches["tracked-feat"].worktree_path.is_some());

        // Cleanup
        wkm_sandbox::git(
            repo.path(),
            &["worktree", "remove", wt_path.to_str().unwrap()],
        );
    }

    #[test]
    fn repair_clears_stale_main_worktree_path() {
        // A branch whose state has worktree_path = Some(main_worktree) but is
        // not currently checked out anywhere is stale. Repair first clears
        // the bogus path (step 5), then prunes the now-empty entry (step 6).
        let (repo, ctx, git) = setup();
        repo.create_branch("stale-main-ref");

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "stale-main-ref".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: Some(ctx.main_worktree.clone()),
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let result = repair(&ctx, &git).unwrap();
        assert!(
            result
                .worktree_paths_cleared
                .contains(&"stale-main-ref".to_string()),
            "repair should clear stale main-worktree path, got: {:?}",
            result.worktree_paths_cleared
        );
        assert!(
            result
                .branches_pruned
                .contains(&"stale-main-ref".to_string()),
            "repair should prune the now-empty entry, got: {:?}",
            result.branches_pruned
        );

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(!wkm_state.branches.contains_key("stale-main-ref"));
    }

    #[test]
    fn repair_clears_main_worktree_path_for_currently_hosted_branch() {
        // Even the branch CURRENTLY checked out in the main worktree must not
        // have worktree_path = Some(main_worktree) in state — main-worktree
        // hosting is inferred at runtime, not stored.
        let (repo, ctx, git) = setup();
        repo.create_branch("hosted");
        wkm_sandbox::git(repo.path(), &["checkout", "hosted"]);

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "hosted".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: Some(ctx.main_worktree.clone()),
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        repair(&ctx, &git).unwrap();

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert_eq!(
            wkm_state.branches["hosted"].worktree_path, None,
            "worktree_path must never equal main_worktree, even for the currently-hosted branch"
        );
    }

    #[test]
    fn repair_auto_adopt_skips_main_worktree_path() {
        // Auto-adopting an untracked branch that lives in the main worktree
        // must not record main_worktree as its worktree_path.
        let (repo, ctx, git) = setup();
        repo.create_branch("untracked-in-main");
        wkm_sandbox::git(repo.path(), &["checkout", "untracked-in-main"]);

        let result = repair(&ctx, &git).unwrap();
        assert!(
            result
                .branches_adopted
                .contains(&"untracked-in-main".to_string()),
            "repair should auto-adopt the untracked branch"
        );

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert_eq!(
            wkm_state.branches["untracked-in-main"].worktree_path, None,
            "auto-adopted main-worktree branch must have worktree_path = None"
        );
    }

    #[test]
    fn repair_does_not_adopt_base_branch() {
        let (_repo, ctx, git) = setup();

        // The main branch is in the main worktree but should NOT be adopted
        let result = repair(&ctx, &git).unwrap();
        assert!(
            result.branches_adopted.is_empty(),
            "Base branch should not be auto-adopted"
        );

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(
            !wkm_state.branches.contains_key("main"),
            "Base branch should not appear in branches map"
        );
    }

    #[test]
    fn repair_prunes_worktreeless_entry() {
        // A tracked branch with no worktree, no stash, not the current main
        // branch is pure legacy clutter — repair removes it from state.
        let (repo, ctx, git) = setup();
        repo.create_branch("dangling");

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "dangling".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: None,
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let result = repair(&ctx, &git).unwrap();
        assert!(
            result.branches_pruned.contains(&"dangling".to_string()),
            "expected 'dangling' in branches_pruned, got: {:?}",
            result.branches_pruned
        );

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(!wkm_state.branches.contains_key("dangling"));
        // Git branch itself is preserved.
        assert!(git.branch_exists("dangling").unwrap());
    }

    #[test]
    fn repair_keeps_entry_with_stash() {
        // An entry with a pending auto-stash must never be pruned — losing
        // the stash_commit SHA would orphan the stash commit (SPEC §8.1).
        let (repo, ctx, git) = setup();
        repo.create_branch("stashed");

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "stashed".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: None,
                stash_commit: Some("deadbeefcafebabe".to_string()),
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let result = repair(&ctx, &git).unwrap();
        assert!(
            !result.branches_pruned.contains(&"stashed".to_string()),
            "entry with pending stash must not be pruned"
        );

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(wkm_state.branches.contains_key("stashed"));
        assert_eq!(
            wkm_state.branches["stashed"].stash_commit.as_deref(),
            Some("deadbeefcafebabe")
        );
    }

    #[test]
    fn repair_keeps_current_main_branch() {
        // The branch currently checked out in the main worktree has
        // worktree_path = None by invariant (SPEC §5.3). Pruning would
        // drop the tracked branch the user is actively working on.
        let (repo, ctx, git) = setup();
        repo.create_branch("hosted");
        wkm_sandbox::git(repo.path(), &["checkout", "hosted"]);

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "hosted".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: None,
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let result = repair(&ctx, &git).unwrap();
        assert!(
            !result.branches_pruned.contains(&"hosted".to_string()),
            "current main-worktree branch must not be pruned"
        );

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(wkm_state.branches.contains_key("hosted"));
    }
}
