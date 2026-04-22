use std::process::Command;

use wkm_sandbox::TestRepo;

fn wkm_bin() -> String {
    env!("CARGO_BIN_EXE_wkm").to_string()
}

fn run(repo: &TestRepo, args: &[&str]) -> std::process::Output {
    Command::new(wkm_bin())
        .args(args)
        .current_dir(repo.path())
        .env_remove("WKM_SHELL_SETUP")
        .output()
        .expect("failed to run wkm")
}

fn run_ok(repo: &TestRepo, args: &[&str]) -> (String, String) {
    let output = run(repo, args);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        output.status.success(),
        "wkm {} failed\nstdout: {stdout}\nstderr: {stderr}",
        args.join(" ")
    );
    (stdout, stderr)
}

fn init(repo: &TestRepo) {
    run_ok(repo, &["init"]);
}

#[test]
fn wp_main_token_returns_main_worktree() {
    let repo = TestRepo::new();
    init(&repo);

    let (stdout, _) = run_ok(&repo, &["wp", "@main"]);
    assert_eq!(stdout.trim(), repo.path().to_string_lossy());
}

#[test]
fn wp_no_arg_returns_main_worktree() {
    let repo = TestRepo::new();
    init(&repo);

    let (stdout, _) = run_ok(&repo, &["wp"]);
    assert_eq!(stdout.trim(), repo.path().to_string_lossy());
}

#[test]
fn wp_rejects_unknown_at_token() {
    let repo = TestRepo::new();
    init(&repo);

    let output = run(&repo, &["wp", "@bogus"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("@bogus") || stderr.contains("Only '@main'"),
        "stderr: {stderr}"
    );
}

#[test]
fn worktree_create_with_name_registers_alias() {
    let repo = TestRepo::new();
    init(&repo);

    let (stdout, _) = run_ok(&repo, &["worktree", "create", "feat", "--name", "specs"]);
    assert!(
        stdout.contains("Workspace alias: specs"),
        "unexpected stdout: {stdout}"
    );

    // Alias resolution via wp.
    let (stdout, _) = run_ok(&repo, &["wp", "specs"]);
    assert!(stdout.trim().ends_with("/repo") || !stdout.trim().is_empty());

    // Alias appears in `workspace list`.
    let (stdout, _) = run_ok(&repo, &["workspace", "list"]);
    assert!(stdout.contains("specs"), "workspace list output: {stdout}");
}

#[test]
fn workspace_rename_and_clear() {
    let repo = TestRepo::new();
    init(&repo);

    run_ok(&repo, &["worktree", "create", "feat", "--name", "specs"]);
    run_ok(&repo, &["workspace", "rename", "specs", "scratch"]);

    let (stdout, _) = run_ok(&repo, &["workspace", "list"]);
    assert!(stdout.contains("scratch"));
    assert!(!stdout.contains("specs"));

    // Clear leaves the worktree but removes the alias.
    let state_path = repo.path().join(".git/wkm.toml");
    let content = std::fs::read_to_string(&state_path).unwrap();
    assert!(content.contains("scratch"));

    run_ok(&repo, &["workspace", "clear", "scratch"]);
    let (stdout, _) = run_ok(&repo, &["workspace", "list"]);
    assert!(!stdout.contains("scratch"));
}

#[test]
fn wp_warns_on_alias_branch_collision() {
    let repo = TestRepo::new();
    init(&repo);

    // A tracked branch literally named `specs`.
    run_ok(&repo, &["worktree", "create", "specs"]);
    // Plus a workspace alias "specs" on a different worktree.
    run_ok(&repo, &["worktree", "create", "other", "--name", "specs"]);

    let output = run(&repo, &["wp", "specs"]);
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("warning:") && stderr.contains("alias"),
        "expected collision warning, got: {stderr}"
    );
}

#[test]
fn wp_force_branch_bypasses_alias() {
    let repo = TestRepo::new();
    init(&repo);

    // A tracked branch named "specs" via worktree create.
    run_ok(&repo, &["worktree", "create", "specs"]);
    // And a workspace alias "specs" pointing at a different worktree.
    run_ok(
        &repo,
        &["worktree", "create", "other-branch", "--name", "specs"],
    );

    let (alias_path, _) = run_ok(&repo, &["wp", "specs"]);
    let (branch_path, _) = run_ok(&repo, &["wp", "-b", "specs"]);
    assert_ne!(
        alias_path.trim(),
        branch_path.trim(),
        "alias and force-branch should resolve to different paths"
    );
}

#[test]
fn worktree_create_rejects_duplicate_alias() {
    let repo = TestRepo::new();
    init(&repo);

    run_ok(&repo, &["worktree", "create", "a", "--name", "specs"]);
    let output = run(&repo, &["worktree", "create", "b", "--name", "specs"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("specs") && stderr.contains("already"),
        "expected duplicate-alias error, got: {stderr}"
    );
}

#[test]
fn worktree_create_rejects_invalid_alias() {
    let repo = TestRepo::new();
    init(&repo);

    for bad in &["@main", "UPPER", "has space", ""] {
        let output = run(&repo, &["worktree", "create", "feat", "--name", bad]);
        if output.status.success() {
            panic!("expected invalid alias '{bad}' to fail");
        }
    }
}
