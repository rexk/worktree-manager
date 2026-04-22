//! End-to-end repair test driven by a hand-crafted state fixture.
//!
//! Simulates the user-reported scenario: a `wkm.toml` accumulated many
//! worktree-less entries (mix of legacy clutter, one pending stash, one
//! current main-worktree branch, one real secondary worktree). Verifies
//! that a single `wkm repair` pass leaves only the entries that genuinely
//! represent current hosting or recoverable work.

use wkm_core::git::cli::CliGit;
use wkm_core::ops::repair;
use wkm_core::repo::RepoContext;
use wkm_core::state;
use wkm_sandbox::TestRepo;

const FIXTURE_TEMPLATE: &str = include_str!("fixtures/dangling_state_after_pr9.toml");

#[test]
fn repair_cleans_dangling_state_fixture() {
    let repo = TestRepo::new();

    // Create every branch the fixture references in real git, so the
    // `branch_exists()` checks in repair don't remove anything prematurely.
    for branch in [
        "dangling-1",
        "dangling-2",
        "dangling-3",
        "stashed-branch",
        "had-bogus-path",
        "hosted",
        "real-feature",
    ] {
        repo.create_branch(branch);
    }

    // Put `hosted` in the main worktree so the current-main safety kicks in.
    repo.checkout("hosted");

    // Create a real secondary worktree for `real-feature`. Use the path git
    // sees (no canonicalize) so platform-specific symlink resolution on
    // macOS (`/var` -> `/private/var`) doesn't cause `worktree_list` to
    // report a differently-encoded path that repair step 5 would overwrite.
    let secondary_parent = tempfile::tempdir().expect("tempdir");
    let real_feature_wt = secondary_parent.path().join("real-feature-wt");
    wkm_sandbox::git(
        repo.path(),
        &[
            "worktree",
            "add",
            real_feature_wt.to_str().unwrap(),
            "real-feature",
        ],
    );

    // Pick a path that definitely does not exist on disk for had-bogus-path.
    let bogus_wt = secondary_parent.path().join("bogus-path-does-not-exist");
    assert!(!bogus_wt.exists());

    // Storage dir for this repo (under the temp dir so the test is hermetic).
    let storage_dir = secondary_parent.path().join("wkm-storage");
    std::fs::create_dir_all(&storage_dir).unwrap();

    // Render the fixture by substituting placeholders. Rendering TOML values
    // with absolute paths via `.display()` is safe here: TempDir paths on all
    // supported platforms contain no characters that need TOML escaping.
    let toml_contents = FIXTURE_TEMPLATE
        .replace("{STORAGE_DIR}", &storage_dir.display().to_string())
        .replace("{REAL_FEATURE_WT}", &real_feature_wt.display().to_string())
        .replace("{BOGUS_WT}", &bogus_wt.display().to_string());

    repo.install_state_fixture(&toml_contents);

    // Resolve context (reads the fixture we just installed) and run repair.
    let ctx = RepoContext::from_path(repo.path()).unwrap();
    let git = CliGit::new(repo.path());
    let result = repair::repair(&ctx, &git).unwrap();

    // `dangling-1/2/3` and `had-bogus-path` should be pruned.
    for branch in ["dangling-1", "dangling-2", "dangling-3", "had-bogus-path"] {
        assert!(
            result.branches_pruned.contains(&branch.to_string()),
            "expected '{branch}' in branches_pruned, got: {:?}",
            result.branches_pruned
        );
    }

    // Step 5 clears the bogus path first, step 6 then prunes.
    assert!(
        result
            .worktree_paths_cleared
            .contains(&"had-bogus-path".to_string()),
        "expected had-bogus-path's missing path to be cleared first"
    );

    // None of the preserved entries should have been pruned.
    for kept in ["stashed-branch", "hosted", "real-feature"] {
        assert!(
            !result.branches_pruned.contains(&kept.to_string()),
            "'{kept}' must not be pruned; got branches_pruned: {:?}",
            result.branches_pruned
        );
    }

    // Verify the post-repair state file directly.
    let wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();

    for gone in ["dangling-1", "dangling-2", "dangling-3", "had-bogus-path"] {
        assert!(
            !wkm_state.branches.contains_key(gone),
            "'{gone}' should be gone from state after repair"
        );
    }

    // Stash survives with its SHA intact.
    assert!(wkm_state.branches.contains_key("stashed-branch"));
    assert_eq!(
        wkm_state.branches["stashed-branch"].stash_commit.as_deref(),
        Some("deadbeefcafebabe0123456789abcdef01234567")
    );
    // Worktree-less (nothing to clear), as loaded.
    assert!(wkm_state.branches["stashed-branch"].worktree_path.is_none());

    // Current main-worktree branch survives unchanged.
    assert!(wkm_state.branches.contains_key("hosted"));
    assert!(wkm_state.branches["hosted"].worktree_path.is_none());

    // Real secondary worktree survives with some worktree_path recorded.
    // Exact equality is avoided: repair step 5 may reconcile the stored
    // path to whatever `git worktree list` reports (which differs from a
    // canonicalized `tempdir()` path on macOS), but the entry must remain
    // and still point at a worktree.
    assert!(wkm_state.branches.contains_key("real-feature"));
    assert!(wkm_state.branches["real-feature"].worktree_path.is_some());

    // Idempotent: a second pass does nothing.
    let result2 = repair::repair(&ctx, &git).unwrap();
    assert!(
        result2.branches_pruned.is_empty(),
        "second repair should not prune anything, got: {:?}",
        result2.branches_pruned
    );
    assert!(result2.branches_removed.is_empty());
    assert!(result2.worktree_paths_cleared.is_empty());
}
