use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// Cross-platform null device path for suppressing git global/system config in tests.
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";
#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";

/// Canonicalize a path for test assertions.
///
/// On Unix, resolves symlinks (e.g. macOS `/var` → `/private/var`).
/// On Windows, resolves 8.3 short names (e.g. `RUNNER~1` → `runneradmin`)
/// without the `\\?\` UNC prefix that [`std::fs::canonicalize`] adds.
pub fn canonicalize(path: &Path) -> PathBuf {
    let p = std::fs::canonicalize(path).expect("failed to canonicalize");
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
    }
    p
}

/// A temporary git repository for testing.
///
/// Creates a fresh git repo with an initial commit on `main`.
/// The repo and all its contents are deleted when dropped.
pub struct TestRepo {
    _dir: TempDir,
    path: PathBuf,
    _remote_dir: Option<TempDir>,
}

impl Default for TestRepo {
    fn default() -> Self {
        Self::new()
    }
}

impl TestRepo {
    /// Create a new git repo with an initial commit on `main`.
    pub fn new() -> Self {
        let dir = TempDir::new().expect("failed to create temp dir");
        let path = canonicalize(dir.path());

        git(&path, &["init", "-b", "main"]);
        git(&path, &["config", "user.name", "Test User"]);
        git(&path, &["config", "user.email", "test@example.com"]);

        let repo = Self {
            _dir: dir,
            path,
            _remote_dir: None,
        };
        repo.commit_file("initial", "initial content", "initial commit");
        repo
    }

    /// Path to the repository root.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Create a file, add it, and commit. Returns the commit hash.
    pub fn commit_file(&self, name: &str, contents: &str, msg: &str) -> String {
        let file_path = self.path.join(name);
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent).expect("failed to create parent dirs");
        }
        std::fs::write(&file_path, contents).expect("failed to write file");
        git(&self.path, &["add", name]);
        git(&self.path, &["commit", "-m", msg]);
        self.head_hash()
    }

    /// Create a branch at HEAD.
    pub fn create_branch(&self, name: &str) {
        git(&self.path, &["branch", name]);
    }

    /// Checkout a branch.
    pub fn checkout(&self, name: &str) {
        git(&self.path, &["checkout", name]);
    }

    /// Get HEAD commit hash.
    pub fn head_hash(&self) -> String {
        git_output(&self.path, &["rev-parse", "HEAD"])
    }

    /// Get the git common dir (e.g. `.git`).
    pub fn git_common_dir(&self) -> PathBuf {
        let out = git_output(&self.path, &["rev-parse", "--git-common-dir"]);
        let p = Path::new(&out);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.path.join(p)
        }
    }

    /// Make the working tree dirty by modifying a tracked file.
    pub fn make_dirty(&self) {
        let file_path = self.path.join("initial");
        std::fs::write(&file_path, "dirty content").expect("failed to write file");
    }

    /// Add an untracked file to the working tree.
    pub fn add_untracked(&self, name: &str) {
        let file_path = self.path.join(name);
        std::fs::write(&file_path, "untracked content").expect("failed to write file");
    }

    /// Stage a change without committing.
    pub fn stage_change(&self, name: &str, contents: &str) {
        let file_path = self.path.join(name);
        std::fs::write(&file_path, contents).expect("failed to write file");
        git(&self.path, &["add", name]);
    }

    /// Set up a bare remote in a sibling temp dir, push main to it.
    /// Returns the path to the bare remote.
    pub fn with_remote(&mut self) -> PathBuf {
        let remote_dir = TempDir::new().expect("failed to create remote temp dir");
        let remote_path = remote_dir.path().to_path_buf();
        git(&remote_path, &["init", "--bare"]);
        git(
            &self.path,
            &["remote", "add", "origin", remote_path.to_str().unwrap()],
        );
        git(&self.path, &["push", "-u", "origin", "main"]);
        self._remote_dir = Some(remote_dir);
        remote_path
    }

    /// Set up a rebase conflict state. Creates divergent commits on two branches
    /// and starts a rebase that will conflict.
    ///
    /// Returns `(base_branch, conflict_branch)` names.
    /// Leaves the repo in a "rebase in progress" state on `conflict_branch`.
    pub fn start_rebase_conflict(&self) -> (String, String) {
        // Create conflicting content on main
        self.commit_file("conflict-file", "main content", "main: add conflict-file");

        // Create a branch with conflicting content
        self.create_branch("conflict-branch");
        // Reset main's file, checkout conflict-branch, add different content
        self.checkout("conflict-branch");
        git(&self.path, &["reset", "--hard", "HEAD~1"]);
        self.commit_file(
            "conflict-file",
            "branch content",
            "branch: add conflict-file",
        );

        // Start rebase — this will conflict
        let output = Command::new("git")
            .args(["rebase", "main"])
            .current_dir(&self.path)
            .env("GIT_CONFIG_GLOBAL", NULL_DEVICE)
            .env("GIT_CONFIG_SYSTEM", NULL_DEVICE)
            .output()
            .expect("failed to run git rebase");
        assert!(
            !output.status.success(),
            "rebase should have conflicted but succeeded"
        );

        ("main".to_string(), "conflict-branch".to_string())
    }
}

/// Run a git command, panicking on failure.
pub fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", NULL_DEVICE)
        .env("GIT_CONFIG_SYSTEM", NULL_DEVICE)
        .output()
        .expect("failed to run git");
    if !output.status.success() {
        panic!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    output
}

/// Run a git command and return stdout, trimmed.
pub fn git_output(dir: &Path, args: &[&str]) -> String {
    let output = git(dir, args);
    String::from_utf8(output.stdout)
        .expect("non-utf8 git output")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_repo_has_initial_commit_on_main() {
        let repo = TestRepo::new();
        let hash = repo.head_hash();
        assert!(!hash.is_empty());
        let branch = git_output(repo.path(), &["branch", "--show-current"]);
        assert_eq!(branch, "main");
    }

    #[test]
    fn commit_file_advances_head() {
        let repo = TestRepo::new();
        let h1 = repo.head_hash();
        let h2 = repo.commit_file("foo.txt", "hello", "add foo");
        assert_ne!(h1, h2);
    }

    #[test]
    fn create_and_checkout_branch() {
        let repo = TestRepo::new();
        repo.create_branch("feature");
        repo.checkout("feature");
        let branch = git_output(repo.path(), &["branch", "--show-current"]);
        assert_eq!(branch, "feature");
    }

    #[test]
    fn make_dirty_modifies_tracked_file() {
        let repo = TestRepo::new();
        repo.make_dirty();
        let status = git_output(repo.path(), &["status", "--porcelain"]);
        assert!(status.contains("initial"));
    }

    #[test]
    fn add_untracked_creates_new_file() {
        let repo = TestRepo::new();
        repo.add_untracked("new-file.txt");
        let status = git_output(repo.path(), &["status", "--porcelain"]);
        assert!(status.contains("?? new-file.txt"));
    }

    #[test]
    fn stage_change_shows_in_index() {
        let repo = TestRepo::new();
        repo.stage_change("staged.txt", "staged content");
        let status = git_output(repo.path(), &["status", "--porcelain"]);
        assert!(status.contains("A  staged.txt"));
    }

    #[test]
    fn with_remote_sets_up_origin() {
        let mut repo = TestRepo::new();
        let remote_path = repo.with_remote();
        assert!(remote_path.exists());
        let remotes = git_output(repo.path(), &["remote", "-v"]);
        assert!(remotes.contains("origin"));
    }

    #[test]
    fn start_rebase_conflict_leaves_rebase_in_progress() {
        let repo = TestRepo::new();
        repo.start_rebase_conflict();
        // Should be in rebase state
        let rebase_dir = repo.path().join(".git/rebase-merge");
        let rebase_dir2 = repo.path().join(".git/rebase-apply");
        assert!(
            rebase_dir.exists() || rebase_dir2.exists(),
            "expected rebase in progress"
        );
    }

    #[test]
    fn git_common_dir_resolves() {
        let repo = TestRepo::new();
        let common = repo.git_common_dir();
        assert!(common.exists());
        assert!(common.join("HEAD").exists());
    }
}
