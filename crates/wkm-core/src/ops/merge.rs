use std::collections::BTreeMap;
use std::path::Path;

use crate::error::WkmError;
use crate::git::types::MergeResult;
use crate::git::{GitBranches, GitDiscovery, GitMutations, GitStash, GitStatus, GitWorktrees};
use crate::graph;
use crate::repo::RepoContext;
use crate::state;
use crate::state::lock::WkmLock;
use crate::state::types::{MergeStrategy, WalEntry, WalOp};

/// Merge a child branch into its parent.
pub fn merge(
    ctx: &RepoContext,
    git: &(impl GitDiscovery + GitBranches + GitWorktrees + GitStatus + GitStash + GitMutations),
    worktree: &Path,
    child_branch: &str,
    strategy: Option<MergeStrategy>,
) -> Result<(), WkmError> {
    let lock = WkmLock::acquire(&ctx.lock_path)?;

    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    if wkm_state.wal.is_some() {
        return Err(WkmError::OperationInProgress);
    }

    let current_branch = git
        .current_branch(worktree)?
        .ok_or_else(|| WkmError::Other("detached HEAD".to_string()))?;

    // Verify child's parent is current branch
    let child_entry = wkm_state
        .branches
        .get(child_branch)
        .ok_or_else(|| WkmError::BranchNotTracked(child_branch.to_string()))?;

    if child_entry.parent.as_deref() != Some(&current_branch) {
        return Err(WkmError::NotAChild(
            child_branch.to_string(),
            current_branch.clone(),
        ));
    }

    // Check for dirty worktrees
    if git.is_dirty(worktree)? {
        return Err(WkmError::DirtyWorktree(current_branch.clone()));
    }
    if let Some(ref wt_path) = child_entry.worktree_path
        && git.is_dirty(wt_path)?
    {
        return Err(WkmError::DirtyWorktree(child_branch.to_string()));
    }

    // Check descendants for dirty worktrees
    let descendants = graph::descendants_of(child_branch, &wkm_state.branches);
    for (desc_name, desc_entry) in &descendants {
        if let Some(ref wt_path) = desc_entry.worktree_path
            && git.is_dirty(wt_path)?
        {
            return Err(WkmError::DirtyWorktree((*desc_name).clone()));
        }
    }

    let merge_strategy = strategy.unwrap_or(wkm_state.config.merge_strategy);

    // Record pre-merge refs for abort
    let parent_ref = git.branch_ref(&current_branch)?;
    let child_ref = git.branch_ref(child_branch)?;
    let descendant_parents: BTreeMap<String, String> = descendants
        .iter()
        .map(|(name, entry)| ((*name).clone(), entry.parent.clone().unwrap_or_default()))
        .collect();

    // Write WAL
    let child_worktree = child_entry.worktree_path.clone();
    wkm_state.wal = Some(WalEntry {
        id: uuid::Uuid::new_v4().to_string(),
        parent_op_id: None,
        op: WalOp::Merge {
            child_branch: child_branch.to_string(),
            parent_ref: parent_ref.clone(),
            child_ref: child_ref.clone(),
            descendant_parents: descendant_parents.clone(),
            worktree_path: child_worktree.clone(),
        },
    });
    state::write_state(&ctx.state_path, &wkm_state)?;

    // Perform merge
    let result = match merge_strategy {
        MergeStrategy::Ff => git.merge_ff_only(worktree, child_branch)?,
        MergeStrategy::MergeCommit => {
            let msg = format!("Merge branch '{child_branch}' into {current_branch}");
            git.merge_no_ff(worktree, child_branch, &msg)?
        }
        MergeStrategy::Squash => git.merge_squash(worktree, child_branch)?,
    };

    match result {
        MergeResult::Clean | MergeResult::UpToDate => {}
        MergeResult::NotFastForward => {
            // Clear WAL since merge didn't happen
            wkm_state.wal = None;
            state::write_state(&ctx.state_path, &wkm_state)?;
            return Err(WkmError::NotFastForward(child_branch.to_string()));
        }
        MergeResult::Conflict { .. } => {
            wkm_state.wal = None;
            state::write_state(&ctx.state_path, &wkm_state)?;
            return Err(WkmError::Conflict(
                child_branch.to_string(),
                "merge conflict".to_string(),
            ));
        }
    }

    // Re-parent descendants to current branch
    let child_children: Vec<String> = graph::children_of(child_branch, &wkm_state.branches)
        .iter()
        .map(|(name, _)| (*name).clone())
        .collect();

    for desc_name in &child_children {
        if let Some(entry) = wkm_state.branches.get_mut(desc_name) {
            entry.parent = Some(current_branch.clone());
        }
    }

    // If this branch's worktree has a workspace alias, park it on a fresh
    // `_wkm/parked/<alias>` branch at the current parent tip so the directory
    // survives the merge for the next iteration.
    let aliased_workspace = child_worktree
        .as_ref()
        .and_then(|wt| crate::ops::workspace::alias_for_path(&wkm_state, wt));

    if let (Some(wt_path), Some(alias)) = (&child_worktree, &aliased_workspace) {
        let parked = format!("_wkm/parked/{alias}");
        let parent_ref_tip = git.branch_ref(&current_branch)?;
        // Create or move _wkm/parked/<alias> to the current parent tip.
        git.force_branch(&parked, &parent_ref_tip)?;
        // Switch the workspace to the parked branch so the merged branch is freed.
        git.checkout(wt_path, &parked)?;
    } else if let Some(ref wt_path) = child_worktree {
        // No alias — preserve existing behaviour: tear the worktree down.
        let _ = git.worktree_remove(wt_path, true);
    }

    // Clean up hold branches
    let hold_branch = format!("_wkm/hold/{child_branch}");
    if git.branch_exists(&hold_branch)? {
        let _ = git.delete_branch(&hold_branch, true);
    }

    // Delete child branch
    let _ = git.delete_branch(child_branch, true);

    // Remove child from state (aliases stay in state.workspaces).
    wkm_state.branches.remove(child_branch);

    // Clear WAL
    wkm_state.wal = None;
    state::write_state(&ctx.state_path, &wkm_state)?;

    drop(lock);
    Ok(())
}

/// Merge all children of the current branch.
pub fn merge_all(
    ctx: &RepoContext,
    git: &(impl GitDiscovery + GitBranches + GitWorktrees + GitStatus + GitStash + GitMutations),
    worktree: &Path,
    strategy: Option<MergeStrategy>,
) -> Result<Vec<String>, WkmError> {
    let lock = WkmLock::acquire(&ctx.lock_path)?;

    let wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    if wkm_state.wal.is_some() {
        return Err(WkmError::OperationInProgress);
    }

    let current_branch = git
        .current_branch(worktree)?
        .ok_or_else(|| WkmError::Other("detached HEAD".to_string()))?;

    let children: Vec<String> = graph::children_of(&current_branch, &wkm_state.branches)
        .iter()
        .map(|(name, _)| (*name).clone())
        .collect();

    if children.is_empty() {
        drop(lock);
        return Ok(vec![]);
    }

    drop(lock);

    let mut merged = Vec::new();

    for child in &children {
        match merge(ctx, git, worktree, child, strategy) {
            Ok(()) => merged.push(child.clone()),
            Err(e) => {
                // Write merge-all WAL with progress so repair can find it
                let lock = WkmLock::acquire(&ctx.lock_path)?;
                let mut wkm_state =
                    state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;
                wkm_state.wal = Some(WalEntry {
                    id: uuid::Uuid::new_v4().to_string(),
                    parent_op_id: None,
                    op: WalOp::MergeAll {
                        children: children.clone(),
                        completed: merged.clone(),
                        pending: children
                            .iter()
                            .filter(|c| !merged.contains(c) && *c != child)
                            .cloned()
                            .collect(),
                    },
                });
                state::write_state(&ctx.state_path, &wkm_state)?;
                drop(lock);

                return Err(e);
            }
        }
    }

    Ok(merged)
}

/// Abort a merge, restoring pre-merge state.
pub fn merge_abort(
    ctx: &RepoContext,
    git: &(impl GitDiscovery + GitBranches + GitWorktrees + GitStatus + GitStash + GitMutations),
) -> Result<(), WkmError> {
    let lock = WkmLock::acquire(&ctx.lock_path)?;

    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    let wal = wkm_state
        .wal
        .as_ref()
        .ok_or(WkmError::NoOperationInProgress)?;

    match &wal.op {
        WalOp::Merge {
            child_branch,
            parent_ref,
            child_ref,
            descendant_parents,
            worktree_path,
        } => {
            // Get parent branch
            let main_wt = git.main_worktree_path()?;
            let parent_branch = git
                .current_branch(&main_wt)?
                .ok_or_else(|| WkmError::Other("detached HEAD".to_string()))?;

            // Reset parent to pre-merge ref
            git.reset_hard(&main_wt, parent_ref)?;

            // Recreate child branch
            if !git.branch_exists(child_branch)? {
                git.create_branch(child_branch, child_ref)?;
            }

            // Restore worktree if it existed
            if let Some(wt_path) = worktree_path
                && !wt_path.exists()
            {
                let _ = git.worktree_add(wt_path, child_branch);
            }

            // Restore child in state
            let now = chrono::Utc::now().to_rfc3339();
            wkm_state.branches.insert(
                child_branch.clone(),
                crate::state::types::BranchEntry {
                    parent: Some(parent_branch),
                    worktree_path: worktree_path.clone(),
                    stash_commit: None,
                    jj_workspace_name: None,
                    description: None,
                    created_at: now,
                    previous_branch: None,
                },
            );

            // Restore descendant parents
            for (desc, parent) in descendant_parents {
                if let Some(entry) = wkm_state.branches.get_mut(desc) {
                    entry.parent = Some(parent.clone());
                }
            }
        }
        _ => return Err(WkmError::NoOperationInProgress),
    }

    wkm_state.wal = None;
    state::write_state(&ctx.state_path, &wkm_state)?;

    drop(lock);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::GitDiscovery;
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
    fn merge_ff_basic() {
        let (repo, ctx, git) = setup();

        repo.create_branch("feature");
        repo.checkout("feature");
        repo.commit_file("feat-file", "feature", "feature commit");
        repo.checkout("main");

        add_branch(&ctx, "feature", "main");

        let main_wt = git.main_worktree_path().unwrap();
        merge(&ctx, &git, &main_wt, "feature", None).unwrap();

        // Branch should be deleted
        assert!(!git.branch_exists("feature").unwrap());

        // State should not have feature
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(!wkm_state.branches.contains_key("feature"));

        // WAL should be cleared
        assert!(wkm_state.wal.is_none());
    }

    #[test]
    fn merge_not_child_errors() {
        let (repo, ctx, git) = setup();
        repo.create_branch("feature");
        repo.create_branch("other");
        repo.checkout("other");

        add_branch(&ctx, "feature", "main");

        // Try to merge from "other" branch
        let result = merge(&ctx, &git, repo.path(), "feature", None);
        assert!(matches!(result, Err(WkmError::NotAChild(_, _))));
    }

    #[test]
    fn merge_dirty_errors() {
        let (repo, ctx, git) = setup();
        repo.create_branch("feature");
        repo.checkout("feature");
        repo.commit_file("feat", "f", "feat commit");
        repo.checkout("main");

        add_branch(&ctx, "feature", "main");

        // Make main dirty
        repo.make_dirty();
        let main_wt = git.main_worktree_path().unwrap();
        let result = merge(&ctx, &git, &main_wt, "feature", None);
        assert!(matches!(result, Err(WkmError::DirtyWorktree(_))));
    }

    #[test]
    fn merge_not_ff_errors() {
        let (repo, ctx, git) = setup();

        repo.create_branch("feature");
        repo.checkout("feature");
        repo.commit_file("feat", "f", "feat commit");
        repo.checkout("main");
        // Advance main divergently
        repo.commit_file("main-file", "m", "main commit");

        add_branch(&ctx, "feature", "main");

        let main_wt = git.main_worktree_path().unwrap();
        let result = merge(&ctx, &git, &main_wt, "feature", Some(MergeStrategy::Ff));
        assert!(matches!(result, Err(WkmError::NotFastForward(_))));
    }

    #[test]
    fn merge_merge_commit_strategy() {
        let (repo, ctx, git) = setup();

        repo.create_branch("feature");
        repo.checkout("feature");
        repo.commit_file("feat", "f", "feat commit");
        repo.checkout("main");
        repo.commit_file("main-file", "m", "main commit");

        add_branch(&ctx, "feature", "main");

        let main_wt = git.main_worktree_path().unwrap();
        merge(
            &ctx,
            &git,
            &main_wt,
            "feature",
            Some(MergeStrategy::MergeCommit),
        )
        .unwrap();

        assert!(!git.branch_exists("feature").unwrap());
    }

    #[test]
    fn merge_squash_strategy() {
        let (repo, ctx, git) = setup();

        repo.create_branch("feature");
        repo.checkout("feature");
        repo.commit_file("feat", "f", "feat commit");
        repo.checkout("main");
        repo.commit_file("main-file", "m", "main commit");

        add_branch(&ctx, "feature", "main");

        let main_wt = git.main_worktree_path().unwrap();
        merge(&ctx, &git, &main_wt, "feature", Some(MergeStrategy::Squash)).unwrap();

        assert!(!git.branch_exists("feature").unwrap());
    }

    #[test]
    fn merge_reparent_descendants() {
        let (repo, ctx, git) = setup();

        // main → feature → sub
        repo.create_branch("feature");
        repo.checkout("feature");
        repo.commit_file("feat", "f", "feat commit");
        repo.create_branch("sub");
        repo.checkout("sub");
        repo.commit_file("sub-file", "s", "sub commit");
        repo.checkout("main");

        add_branch(&ctx, "feature", "main");
        add_branch(&ctx, "sub", "feature");

        let main_wt = git.main_worktree_path().unwrap();
        merge(&ctx, &git, &main_wt, "feature", None).unwrap();

        // sub should now have main as parent
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert_eq!(wkm_state.branches["sub"].parent, Some("main".to_string()));
    }

    #[test]
    fn merge_cleanup_state() {
        let (repo, ctx, git) = setup();

        repo.create_branch("feature");
        repo.checkout("feature");
        repo.commit_file("feat", "f", "feat commit");
        repo.checkout("main");

        add_branch(&ctx, "feature", "main");

        let main_wt = git.main_worktree_path().unwrap();
        merge(&ctx, &git, &main_wt, "feature", None).unwrap();

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(!wkm_state.branches.contains_key("feature"));
        assert!(wkm_state.wal.is_none());
    }

    #[test]
    fn merge_all_sequential() {
        let (repo, ctx, git) = setup();

        // Three children of main
        for name in &["child-a", "child-b", "child-c"] {
            repo.create_branch(name);
            repo.checkout(name);
            repo.commit_file(&format!("{name}-file"), name, &format!("{name} commit"));
            repo.checkout("main");
            add_branch(&ctx, name, "main");
        }

        let main_wt = git.main_worktree_path().unwrap();
        let merged = merge_all(&ctx, &git, &main_wt, Some(MergeStrategy::MergeCommit)).unwrap();
        assert_eq!(merged.len(), 3);

        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(!wkm_state.branches.contains_key("child-a"));
        assert!(!wkm_state.branches.contains_key("child-b"));
        assert!(!wkm_state.branches.contains_key("child-c"));
    }

    #[test]
    fn merge_with_alias_parks_worktree() {
        use crate::ops::worktree::{self, CreateOptions};

        let (_repo, ctx, git) = setup();

        // Create a worktree with an alias, add a commit, then merge.
        let created = worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feat".to_string(),
                base: None,
                description: None,
                name: Some("specs".to_string()),
            },
        )
        .unwrap();
        let wt_path = created.worktree_path.clone();

        // Create a commit in the worktree so the merge has real work.
        std::fs::write(wt_path.join("feat-file"), "content").unwrap();
        wkm_sandbox::git(&wt_path, &["add", "."]);
        wkm_sandbox::git(&wt_path, &["commit", "-m", "feat commit"]);

        let main_wt = git.main_worktree_path().unwrap();
        merge(&ctx, &git, &main_wt, "feat", None).unwrap();

        // Worktree survives.
        assert!(wt_path.exists(), "worktree directory should persist");

        // Worktree is parked on _wkm/parked/specs.
        let current = git.current_branch(&wt_path).unwrap();
        assert_eq!(current.as_deref(), Some("_wkm/parked/specs"));
        assert!(git.branch_exists("_wkm/parked/specs").unwrap());

        // Merged branch is gone.
        assert!(!git.branch_exists("feat").unwrap());

        // Alias entry still points at the worktree.
        let state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(state.workspaces.contains_key("specs"));
        assert_eq!(state.workspaces["specs"].worktree_path, wt_path);
        // BranchEntry for the merged branch is removed.
        assert!(!state.branches.contains_key("feat"));
    }

    #[test]
    fn merge_abort_restores() {
        let (repo, ctx, git) = setup();

        repo.create_branch("feature");
        repo.checkout("feature");
        repo.commit_file("feat", "f", "feat commit");
        repo.checkout("main");

        add_branch(&ctx, "feature", "main");

        let main_wt = git.main_worktree_path().unwrap();
        let pre_main_ref = git.branch_ref("main").unwrap();

        merge(&ctx, &git, &main_wt, "feature", None).unwrap();

        // Feature is now merged — write a fake WAL as if we need to abort
        // In real usage, abort would be called before WAL is cleared
        // Let's test abort with a manually set WAL
        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        let child_ref = wkm_sandbox::git_output(repo.path(), &["rev-parse", "HEAD"]);
        wkm_state.wal = Some(WalEntry {
            id: "test".to_string(),
            parent_op_id: None,
            op: WalOp::Merge {
                child_branch: "feature".to_string(),
                parent_ref: pre_main_ref.clone(),
                child_ref: child_ref.clone(),
                descendant_parents: BTreeMap::new(),
                worktree_path: None,
            },
        });
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        merge_abort(&ctx, &git).unwrap();

        // Main should be restored
        let post_main_ref = git.branch_ref("main").unwrap();
        assert_eq!(pre_main_ref, post_main_ref);

        // Feature branch should be recreated
        assert!(git.branch_exists("feature").unwrap());

        // WAL should be cleared
        let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(wkm_state.wal.is_none());
    }
}
