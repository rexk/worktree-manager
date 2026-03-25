use std::process::Command;

use wkm_sandbox::TestRepo;

fn wkm_bin() -> String {
    env!("CARGO_BIN_EXE_wkm").to_string()
}

/// Initialize wkm in a test repo and add a branch with a worktree path to state.
fn setup_with_branch(branch: &str, worktree_path: &std::path::Path) -> TestRepo {
    let repo = TestRepo::new();

    // Initialize wkm
    let output = Command::new(wkm_bin())
        .args(["init"])
        .current_dir(repo.path())
        .output()
        .expect("failed to run wkm init");
    assert!(
        output.status.success(),
        "wkm init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Read current state, add the branch, write it back
    let state_path = repo.path().join(".git/wkm.toml");
    let content = std::fs::read_to_string(&state_path).expect("failed to read state");
    let mut doc: toml::Table = content.parse().expect("failed to parse toml");

    let mut branch_entry = toml::Table::new();
    branch_entry.insert(
        "parent".to_string(),
        toml::Value::String("main".to_string()),
    );
    branch_entry.insert(
        "worktree_path".to_string(),
        toml::Value::String(worktree_path.to_string_lossy().to_string()),
    );
    branch_entry.insert(
        "created_at".to_string(),
        toml::Value::String("2026-01-01T00:00:00Z".to_string()),
    );

    let branches = doc
        .entry("branches")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .unwrap();
    branches.insert(branch.to_string(), toml::Value::Table(branch_entry));

    std::fs::write(&state_path, doc.to_string()).expect("failed to write state");

    repo
}

#[test]
fn worktree_path_stdout_contains_only_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let wt_path = tmp.path().join("my-worktree");
    std::fs::create_dir_all(&wt_path).unwrap();

    let repo = setup_with_branch("feature", &wt_path);

    let output = Command::new(wkm_bin())
        .args(["worktree-path", "feature"])
        .current_dir(repo.path())
        .output()
        .expect("failed to run wkm");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    // stdout should be just the path with a trailing newline
    assert_eq!(stdout.trim(), wt_path.to_string_lossy());
    // No hint on stderr when invoked as `worktree-path`
    assert!(
        !stderr.contains("hint:"),
        "unexpected hint on stderr: {stderr}"
    );
}

#[test]
fn worktree_path_no_hint_on_stderr() {
    let tmp = tempfile::TempDir::new().unwrap();
    let wt_path = tmp.path().join("my-worktree");
    std::fs::create_dir_all(&wt_path).unwrap();

    let repo = setup_with_branch("feature", &wt_path);

    let output = Command::new(wkm_bin())
        .args(["worktree-path", "feature"])
        .current_dir(repo.path())
        .output()
        .expect("failed to run wkm");

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !stderr.contains("hint:"),
        "worktree-path should not print hint, got: {stderr}"
    );
}

#[test]
fn wp_prints_hint_on_stderr() {
    let tmp = tempfile::TempDir::new().unwrap();
    let wt_path = tmp.path().join("my-worktree");
    std::fs::create_dir_all(&wt_path).unwrap();

    let repo = setup_with_branch("feature", &wt_path);

    let output = Command::new(wkm_bin())
        .args(["wp", "feature"])
        .current_dir(repo.path())
        // Ensure WKM_SHELL_SETUP is NOT set so hint appears
        .env_remove("WKM_SHELL_SETUP")
        .output()
        .expect("failed to run wkm");

    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    // stdout should still be just the path
    assert_eq!(stdout.trim(), wt_path.to_string_lossy());
    // stderr should contain the hint
    assert!(
        stderr.contains("hint:"),
        "expected hint on stderr, got: {stderr}"
    );
    assert!(
        !stdout.contains("hint:"),
        "hint should not be on stdout: {stdout}"
    );
}

#[test]
fn wp_no_hint_when_shell_setup_set() {
    let tmp = tempfile::TempDir::new().unwrap();
    let wt_path = tmp.path().join("my-worktree");
    std::fs::create_dir_all(&wt_path).unwrap();

    let repo = setup_with_branch("feature", &wt_path);

    let output = Command::new(wkm_bin())
        .args(["wp", "feature"])
        .current_dir(repo.path())
        .env("WKM_SHELL_SETUP", "1")
        .output()
        .expect("failed to run wkm");

    assert!(output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !stderr.contains("hint:"),
        "hint should be suppressed when WKM_SHELL_SETUP is set, got: {stderr}"
    );
}

#[test]
fn worktree_path_nonexistent_dir_returns_error() {
    let repo = setup_with_branch(
        "feature",
        std::path::Path::new("/tmp/nonexistent-wkm-test-path-99999"),
    );

    let output = Command::new(wkm_bin())
        .args(["worktree-path", "feature"])
        .current_dir(repo.path())
        .output()
        .expect("failed to run wkm");

    assert!(
        !output.status.success(),
        "should fail when worktree dir doesn't exist"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("no longer exists") || stderr.contains("repair"),
        "error should mention missing path or repair, got: {stderr}"
    );
}

#[test]
fn worktree_path_with_slash_in_branch_name() {
    let tmp = tempfile::TempDir::new().unwrap();
    let wt_path = tmp.path().join("my-worktree");
    std::fs::create_dir_all(&wt_path).unwrap();

    let repo = setup_with_branch("cursor/build-cache-cleanup-8644", &wt_path);

    let output = Command::new(wkm_bin())
        .args(["worktree-path", "cursor/build-cache-cleanup-8644"])
        .current_dir(repo.path())
        .output()
        .expect("failed to run wkm");

    assert!(
        output.status.success(),
        "failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.trim(), wt_path.to_string_lossy());
}
