use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::WkmError;

use super::types::{InProgressOp, MergeResult, RebaseResult, StashEntry, WorktreeInfo};
use super::{GitBranches, GitDiscovery, GitMutations, GitStash, GitStatus, GitWorktrees, Result};

/// Git backend that shells out to the `git` CLI.
pub struct CliGit {
    /// Working directory for discovery commands.
    work_dir: PathBuf,
}

impl CliGit {
    pub fn new(work_dir: impl Into<PathBuf>) -> Self {
        Self {
            work_dir: work_dir.into(),
        }
    }

    /// Run a git command in the default work_dir.
    fn run(&self, args: &[&str]) -> Result<std::process::Output> {
        self.run_in(&self.work_dir, args)
    }

    /// Run a git command in a specific directory.
    fn run_in(&self, dir: &Path, args: &[&str]) -> Result<std::process::Output> {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .map_err(|e| WkmError::Git(format!("failed to run git: {e}")))?;
        Ok(output)
    }

    /// Run a git command and return stdout, or error if it fails.
    fn run_ok(&self, args: &[&str]) -> Result<String> {
        self.run_ok_in(&self.work_dir, args)
    }

    /// Run a git command in a directory and return stdout, or error if it fails.
    fn run_ok_in(&self, dir: &Path, args: &[&str]) -> Result<String> {
        let output = self.run_in(dir, args)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(WkmError::Git(format!(
                "git {} failed: {}",
                args.join(" "),
                stderr.trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Run git -C <dir> with args.
    #[allow(dead_code)]
    fn git_c(&self, dir: &Path, args: &[&str]) -> Result<String> {
        let dir_str = dir
            .to_str()
            .ok_or_else(|| WkmError::Git(format!("non-utf8 path: {}", dir.display())))?;
        let mut full_args = vec!["-C", dir_str];
        full_args.extend_from_slice(args);
        let output = Command::new("git")
            .args(&full_args)
            .output()
            .map_err(|e| WkmError::Git(format!("failed to run git: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(WkmError::Git(format!(
                "git -C {} {} failed: {}",
                dir.display(),
                args.join(" "),
                stderr.trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

impl GitDiscovery for CliGit {
    fn git_common_dir(&self) -> Result<PathBuf> {
        let out = self.run_ok(&["rev-parse", "--git-common-dir"])?;
        let p = Path::new(&out);
        if p.is_absolute() {
            Ok(p.to_path_buf())
        } else {
            Ok(self.work_dir.join(p))
        }
    }

    fn main_worktree_path(&self) -> Result<PathBuf> {
        let output = self.run_ok(&["worktree", "list", "--porcelain"])?;
        // First worktree entry is the main one
        for line in output.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                return Ok(PathBuf::from(path));
            }
        }
        Err(WkmError::Git(
            "could not determine main worktree".to_string(),
        ))
    }

    fn current_branch(&self, worktree: &Path) -> Result<Option<String>> {
        let output = self.run_in(worktree, &["symbolic-ref", "--short", "HEAD"])?;
        if output.status.success() {
            let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if branch.is_empty() {
                Ok(None)
            } else {
                Ok(Some(branch))
            }
        } else {
            // Detached HEAD
            Ok(None)
        }
    }
}

impl GitBranches for CliGit {
    fn branch_exists(&self, name: &str) -> Result<bool> {
        let output = self.run(&["rev-parse", "--verify", &format!("refs/heads/{name}")])?;
        Ok(output.status.success())
    }

    fn create_branch(&self, name: &str, start_point: &str) -> Result<()> {
        self.run_ok(&["branch", name, start_point])?;
        Ok(())
    }

    fn delete_branch(&self, name: &str, force: bool) -> Result<()> {
        let flag = if force { "-D" } else { "-d" };
        self.run_ok(&["branch", flag, name])?;
        Ok(())
    }

    fn force_branch(&self, name: &str, commit: &str) -> Result<()> {
        self.run_ok(&["branch", "-f", name, commit])?;
        Ok(())
    }

    fn branch_ref(&self, name: &str) -> Result<String> {
        self.run_ok(&["rev-parse", &format!("refs/heads/{name}")])
    }

    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        let output = self.run(&["merge-base", "--is-ancestor", ancestor, descendant])?;
        Ok(output.status.success())
    }

    fn ahead_behind(&self, a: &str, b: &str) -> Result<(usize, usize)> {
        let out = self.run_ok(&["rev-list", "--count", "--left-right", &format!("{a}...{b}")])?;
        let parts: Vec<&str> = out.split_whitespace().collect();
        if parts.len() != 2 {
            return Err(WkmError::Git(format!("unexpected rev-list output: {out}")));
        }
        let ahead: usize = parts[0]
            .parse()
            .map_err(|_| WkmError::Git(format!("bad count: {}", parts[0])))?;
        let behind: usize = parts[1]
            .parse()
            .map_err(|_| WkmError::Git(format!("bad count: {}", parts[1])))?;
        Ok((ahead, behind))
    }

    fn branch_list(&self) -> Result<Vec<String>> {
        let output = self.run_ok(&["branch", "--format=%(refname:short)"])?;
        Ok(output
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect())
    }

    fn remote_tracking_branch(&self, branch: &str) -> Result<Option<String>> {
        let output = self.run(&[
            "rev-parse",
            "--abbrev-ref",
            &format!("{branch}@{{upstream}}"),
        ])?;
        if output.status.success() {
            let upstream = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if upstream.is_empty() {
                Ok(None)
            } else {
                Ok(Some(upstream))
            }
        } else {
            Ok(None)
        }
    }
}

impl GitWorktrees for CliGit {
    fn worktree_list(&self) -> Result<Vec<WorktreeInfo>> {
        let output = self.run_ok(&["worktree", "list", "--porcelain"])?;
        let mut worktrees = Vec::new();
        let mut current: Option<WorktreeInfo> = None;

        for line in output.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                if let Some(wt) = current.take() {
                    worktrees.push(wt);
                }
                current = Some(WorktreeInfo {
                    path: PathBuf::from(path),
                    head: String::new(),
                    branch: None,
                    is_bare: false,
                });
            } else if let Some(head) = line.strip_prefix("HEAD ") {
                if let Some(ref mut wt) = current {
                    wt.head = head.to_string();
                }
            } else if let Some(branch) = line.strip_prefix("branch ") {
                if let Some(ref mut wt) = current {
                    // branch comes as refs/heads/name
                    wt.branch = Some(
                        branch
                            .strip_prefix("refs/heads/")
                            .unwrap_or(branch)
                            .to_string(),
                    );
                }
            } else if line == "bare"
                && let Some(ref mut wt) = current
            {
                wt.is_bare = true;
            }
        }
        if let Some(wt) = current {
            worktrees.push(wt);
        }
        Ok(worktrees)
    }

    fn worktree_add(&self, path: &Path, branch: &str) -> Result<()> {
        let path_str = path
            .to_str()
            .ok_or_else(|| WkmError::Git(format!("non-utf8 path: {}", path.display())))?;
        self.run_ok(&["worktree", "add", path_str, branch])?;
        Ok(())
    }

    fn worktree_remove(&self, path: &Path, force: bool) -> Result<()> {
        let path_str = path
            .to_str()
            .ok_or_else(|| WkmError::Git(format!("non-utf8 path: {}", path.display())))?;
        if force {
            self.run_ok(&["worktree", "remove", "--force", path_str])?;
        } else {
            self.run_ok(&["worktree", "remove", path_str])?;
        }
        Ok(())
    }

    fn worktree_repair(&self) -> Result<()> {
        self.run_ok(&["worktree", "repair"])?;
        Ok(())
    }

    fn worktree_prune(&self) -> Result<()> {
        self.run_ok(&["worktree", "prune"])?;
        Ok(())
    }
}

impl GitStatus for CliGit {
    fn is_dirty(&self, worktree: &Path) -> Result<bool> {
        // Dirty = staged changes OR unstaged changes to tracked files.
        // Untracked files alone do NOT count as dirty.
        let output =
            self.run_ok_in(worktree, &["status", "--porcelain", "--untracked-files=no"])?;
        if !output.is_empty() {
            return Ok(true);
        }
        // Also check for in-progress operations
        if self.has_in_progress_operation(worktree)?.is_some() {
            return Ok(true);
        }
        Ok(false)
    }

    fn has_changes_for_stash(&self, worktree: &Path) -> Result<bool> {
        // Stash needs at least staged or unstaged tracked changes
        let output =
            self.run_ok_in(worktree, &["status", "--porcelain", "--untracked-files=no"])?;
        Ok(!output.is_empty())
    }

    fn has_in_progress_operation(&self, worktree: &Path) -> Result<Option<InProgressOp>> {
        // Check for various in-progress git operations
        let git_dir = self.run_ok_in(worktree, &["rev-parse", "--git-dir"])?;
        let git_dir = if Path::new(&git_dir).is_absolute() {
            PathBuf::from(&git_dir)
        } else {
            worktree.join(&git_dir)
        };

        if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
            return Ok(Some(InProgressOp::Rebase));
        }
        if git_dir.join("MERGE_HEAD").exists() {
            return Ok(Some(InProgressOp::Merge));
        }
        if git_dir.join("CHERRY_PICK_HEAD").exists() {
            return Ok(Some(InProgressOp::CherryPick));
        }
        if git_dir.join("REVERT_HEAD").exists() {
            return Ok(Some(InProgressOp::Revert));
        }
        if git_dir.join("BISECT_LOG").exists() {
            return Ok(Some(InProgressOp::Bisect));
        }
        Ok(None)
    }
}

impl GitStash for CliGit {
    fn stash_push(
        &self,
        worktree: &Path,
        message: &str,
        include_untracked: bool,
    ) -> Result<String> {
        let mut args = vec!["stash", "push", "-m", message];
        if include_untracked {
            args.push("--include-untracked");
        }
        self.run_ok_in(worktree, &args)?;

        // Get the stash commit hash (stash@{0})
        let hash = self.run_ok_in(worktree, &["rev-parse", "stash@{0}"])?;
        Ok(hash)
    }

    fn stash_apply(&self, worktree: &Path, commit: &str, index: bool) -> Result<()> {
        let mut args = vec!["stash", "apply"];
        if index {
            args.push("--index");
        }
        args.push(commit);
        self.run_ok_in(worktree, &args)?;
        Ok(())
    }

    fn stash_list(&self) -> Result<Vec<StashEntry>> {
        let output = self.run_ok(&["stash", "list", "--format=%H %s"])?;
        let mut entries = Vec::new();
        for (i, line) in output.lines().enumerate() {
            if let Some((hash, message)) = line.split_once(' ') {
                entries.push(StashEntry {
                    index: i,
                    commit: hash.to_string(),
                    message: message.to_string(),
                });
            }
        }
        Ok(entries)
    }

    fn stash_drop_by_index(&self, index: usize) -> Result<()> {
        self.run_ok(&["stash", "drop", &format!("stash@{{{index}}}")])?;
        Ok(())
    }
}

impl GitMutations for CliGit {
    fn checkout(&self, worktree: &Path, branch: &str) -> Result<()> {
        self.run_ok_in(worktree, &["checkout", branch])?;
        Ok(())
    }

    fn checkout_new_branch(&self, worktree: &Path, name: &str) -> Result<()> {
        self.run_ok_in(worktree, &["checkout", "-b", name])?;
        Ok(())
    }

    fn rebase(&self, worktree: &Path, onto: &str) -> Result<RebaseResult> {
        let output = self.run_in(worktree, &["rebase", onto])?;
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("is up to date") || stdout.contains("up to date") {
                return Ok(RebaseResult::UpToDate);
            }
            return Ok(RebaseResult::Clean);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("CONFLICT") || stderr.contains("could not apply") {
            let conflicted = self.get_conflicted_files(worktree)?;
            return Ok(RebaseResult::Conflict {
                conflicted_files: conflicted,
            });
        }
        Err(WkmError::Git(format!("rebase failed: {}", stderr.trim())))
    }

    fn rebase_continue(&self, worktree: &Path) -> Result<RebaseResult> {
        let output = self.run_in(
            worktree,
            &["-c", "core.editor=true", "rebase", "--continue"],
        )?;
        if output.status.success() {
            return Ok(RebaseResult::Clean);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("CONFLICT") || stderr.contains("could not apply") {
            let conflicted = self.get_conflicted_files(worktree)?;
            return Ok(RebaseResult::Conflict {
                conflicted_files: conflicted,
            });
        }
        Err(WkmError::Git(format!(
            "rebase --continue failed: {}",
            stderr.trim()
        )))
    }

    fn rebase_abort(&self, worktree: &Path) -> Result<()> {
        self.run_ok_in(worktree, &["rebase", "--abort"])?;
        Ok(())
    }

    fn merge_ff_only(&self, worktree: &Path, branch: &str) -> Result<MergeResult> {
        let output = self.run_in(worktree, &["merge", "--ff-only", branch])?;
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("Already up to date") {
                return Ok(MergeResult::UpToDate);
            }
            return Ok(MergeResult::Clean);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("Not possible to fast-forward") || stderr.contains("fatal") {
            return Ok(MergeResult::NotFastForward);
        }
        Err(WkmError::Git(format!(
            "merge --ff-only failed: {}",
            stderr.trim()
        )))
    }

    fn merge_no_ff(&self, worktree: &Path, branch: &str, msg: &str) -> Result<MergeResult> {
        let output = self.run_in(worktree, &["merge", "--no-ff", "-m", msg, branch])?;
        if output.status.success() {
            return Ok(MergeResult::Clean);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("CONFLICT") {
            let conflicted = self.get_conflicted_files(worktree)?;
            return Ok(MergeResult::Conflict {
                conflicted_files: conflicted,
            });
        }
        Err(WkmError::Git(format!(
            "merge --no-ff failed: {}",
            stderr.trim()
        )))
    }

    fn merge_squash(&self, worktree: &Path, branch: &str) -> Result<MergeResult> {
        let output = self.run_in(worktree, &["merge", "--squash", branch])?;
        if output.status.success() {
            // Squash merge stages but doesn't commit
            self.run_ok_in(
                worktree,
                &[
                    "commit",
                    "--no-edit",
                    "-m",
                    &format!("Squash merge {branch}"),
                ],
            )?;
            return Ok(MergeResult::Clean);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("CONFLICT") {
            let conflicted = self.get_conflicted_files(worktree)?;
            return Ok(MergeResult::Conflict {
                conflicted_files: conflicted,
            });
        }
        Err(WkmError::Git(format!(
            "merge --squash failed: {}",
            stderr.trim()
        )))
    }

    fn fetch(&self, remote: &str) -> Result<()> {
        self.run_ok(&["fetch", remote])?;
        Ok(())
    }

    fn reset_hard(&self, worktree: &Path, commit: &str) -> Result<()> {
        self.run_ok_in(worktree, &["reset", "--hard", commit])?;
        Ok(())
    }
}

impl CliGit {
    fn get_conflicted_files(&self, worktree: &Path) -> Result<Vec<String>> {
        let output = self.run_ok_in(worktree, &["diff", "--name-only", "--diff-filter=U"])?;
        Ok(output.lines().map(|s| s.to_string()).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::types::InProgressOp;

    #[cfg(not(windows))]
    const NULL_DEVICE: &str = "/dev/null";
    #[cfg(windows)]
    const NULL_DEVICE: &str = "NUL";

    fn test_repo() -> (tempfile::TempDir, CliGit) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        run_git(&path, &["init", "-b", "main"]);
        run_git(&path, &["config", "user.name", "Test"]);
        run_git(&path, &["config", "user.email", "test@test.com"]);
        run_git(&path, &["config", "commit.gpgsign", "false"]);
        std::fs::write(path.join("initial"), "content").unwrap();
        run_git(&path, &["add", "."]);
        run_git(&path, &["commit", "-m", "initial"]);
        let git = CliGit::new(&path);
        (dir, git)
    }

    fn run_git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", NULL_DEVICE)
            .env("GIT_CONFIG_SYSTEM", NULL_DEVICE)
            .output()
            .unwrap();
        if !output.status.success() {
            panic!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    fn run_git_output(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", NULL_DEVICE)
            .env("GIT_CONFIG_SYSTEM", NULL_DEVICE)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    // --- GitDiscovery ---

    #[test]
    fn discovery_git_common_dir() {
        let (dir, git) = test_repo();
        let common = git.git_common_dir().unwrap();
        assert!(common.exists());
        assert_eq!(common, dir.path().join(".git"));
    }

    #[test]
    fn discovery_main_worktree_path() {
        let (dir, git) = test_repo();
        let main_path = git.main_worktree_path().unwrap();
        let expected = wkm_sandbox::canonicalize(dir.path());
        assert_eq!(main_path, expected);
    }

    #[test]
    fn discovery_current_branch() {
        let (dir, git) = test_repo();
        let branch = git.current_branch(dir.path()).unwrap();
        assert_eq!(branch, Some("main".to_string()));
    }

    #[test]
    fn discovery_current_branch_detached() {
        let (dir, git) = test_repo();
        let hash = run_git_output(dir.path(), &["rev-parse", "HEAD"]);
        run_git(dir.path(), &["checkout", "--detach", &hash]);
        let branch = git.current_branch(dir.path()).unwrap();
        assert_eq!(branch, None);
    }

    // --- GitBranches ---

    #[test]
    fn branches_exists() {
        let (_dir, git) = test_repo();
        assert!(git.branch_exists("main").unwrap());
        assert!(!git.branch_exists("nonexistent").unwrap());
    }

    #[test]
    fn branches_create_and_delete() {
        let (_dir, git) = test_repo();
        git.create_branch("feature", "main").unwrap();
        assert!(git.branch_exists("feature").unwrap());
        git.delete_branch("feature", false).unwrap();
        assert!(!git.branch_exists("feature").unwrap());
    }

    #[test]
    fn branches_ref() {
        let (dir, git) = test_repo();
        let hash = git.branch_ref("main").unwrap();
        let expected = run_git_output(dir.path(), &["rev-parse", "refs/heads/main"]);
        assert_eq!(hash, expected);
    }

    #[test]
    fn branches_is_ancestor() {
        let (dir, git) = test_repo();
        let parent_hash = run_git_output(dir.path(), &["rev-parse", "HEAD"]);
        run_git(dir.path(), &["checkout", "-b", "child"]);
        std::fs::write(dir.path().join("child-file"), "child").unwrap();
        run_git(dir.path(), &["add", "."]);
        run_git(dir.path(), &["commit", "-m", "child commit"]);

        assert!(git.is_ancestor(&parent_hash, "child").unwrap());
        assert!(!git.is_ancestor("child", &parent_hash).unwrap());
    }

    #[test]
    fn branches_ahead_behind() {
        let (dir, git) = test_repo();
        run_git(dir.path(), &["checkout", "-b", "feature"]);
        std::fs::write(dir.path().join("f1"), "1").unwrap();
        run_git(dir.path(), &["add", "."]);
        run_git(dir.path(), &["commit", "-m", "f1"]);
        std::fs::write(dir.path().join("f2"), "2").unwrap();
        run_git(dir.path(), &["add", "."]);
        run_git(dir.path(), &["commit", "-m", "f2"]);

        let (ahead, behind) = git.ahead_behind("feature", "main").unwrap();
        assert_eq!(ahead, 2);
        assert_eq!(behind, 0);
    }

    #[test]
    fn branches_list() {
        let (dir, git) = test_repo();
        run_git(dir.path(), &["branch", "alpha", "main"]);
        run_git(dir.path(), &["branch", "beta", "main"]);
        let mut branches = git.branch_list().unwrap();
        branches.sort();
        assert_eq!(branches, vec!["alpha", "beta", "main"]);
    }

    #[test]
    fn branches_force() {
        let (dir, git) = test_repo();
        let orig = git.branch_ref("main").unwrap();
        run_git(dir.path(), &["checkout", "-b", "tmp"]);
        std::fs::write(dir.path().join("new"), "x").unwrap();
        run_git(dir.path(), &["add", "."]);
        run_git(dir.path(), &["commit", "-m", "new"]);
        let new_hash = run_git_output(dir.path(), &["rev-parse", "HEAD"]);

        git.force_branch("main", &new_hash).unwrap();
        assert_eq!(git.branch_ref("main").unwrap(), new_hash);
        assert_ne!(orig, new_hash);
    }

    // --- GitWorktrees ---

    #[test]
    fn worktrees_list() {
        let (_dir, git) = test_repo();
        let list = git.worktree_list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn worktrees_add_and_remove() {
        let (dir, git) = test_repo();
        git.create_branch("wt-branch", "main").unwrap();
        let wt_path = dir.path().join("wt1");
        git.worktree_add(&wt_path, "wt-branch").unwrap();

        let list = git.worktree_list().unwrap();
        assert_eq!(list.len(), 2);

        git.worktree_remove(&wt_path, false).unwrap();
        let list = git.worktree_list().unwrap();
        assert_eq!(list.len(), 1);
    }

    // --- GitStatus ---

    #[test]
    fn status_clean() {
        let (dir, git) = test_repo();
        assert!(!git.is_dirty(dir.path()).unwrap());
    }

    #[test]
    fn status_dirty_unstaged() {
        let (dir, git) = test_repo();
        std::fs::write(dir.path().join("initial"), "modified").unwrap();
        assert!(git.is_dirty(dir.path()).unwrap());
    }

    #[test]
    fn status_dirty_staged() {
        let (dir, git) = test_repo();
        std::fs::write(dir.path().join("new-file"), "new").unwrap();
        run_git(dir.path(), &["add", "new-file"]);
        assert!(git.is_dirty(dir.path()).unwrap());
    }

    #[test]
    fn status_untracked_not_dirty() {
        let (dir, git) = test_repo();
        std::fs::write(dir.path().join("untracked"), "data").unwrap();
        assert!(!git.is_dirty(dir.path()).unwrap());
    }

    #[test]
    fn status_in_progress_rebase() {
        let (dir, git) = test_repo();
        // Create a conflict
        std::fs::write(dir.path().join("conflict"), "main").unwrap();
        run_git(dir.path(), &["add", "."]);
        run_git(dir.path(), &["commit", "-m", "main conflict"]);
        run_git(dir.path(), &["checkout", "-b", "conflict-br"]);
        run_git(dir.path(), &["reset", "--hard", "HEAD~1"]);
        std::fs::write(dir.path().join("conflict"), "branch").unwrap();
        run_git(dir.path(), &["add", "."]);
        run_git(dir.path(), &["commit", "-m", "branch conflict"]);
        // Start rebase (will fail)
        let _ = Command::new("git")
            .args(["rebase", "main"])
            .current_dir(dir.path())
            .env("GIT_CONFIG_GLOBAL", NULL_DEVICE)
            .env("GIT_CONFIG_SYSTEM", NULL_DEVICE)
            .output();

        let op = git.has_in_progress_operation(dir.path()).unwrap();
        assert_eq!(op, Some(InProgressOp::Rebase));
    }

    // --- GitStash ---

    #[test]
    fn stash_push_and_apply() {
        let (dir, git) = test_repo();
        std::fs::write(dir.path().join("initial"), "modified").unwrap();
        let hash = git
            .stash_push(dir.path(), "wkm: test stash", false)
            .unwrap();
        assert!(!hash.is_empty());
        // Working tree should be clean now
        assert!(!git.is_dirty(dir.path()).unwrap());
        // Apply it back
        git.stash_apply(dir.path(), &hash, false).unwrap();
        assert!(git.is_dirty(dir.path()).unwrap());
    }

    #[test]
    fn stash_list_entries() {
        let (dir, git) = test_repo();
        std::fs::write(dir.path().join("initial"), "v1").unwrap();
        git.stash_push(dir.path(), "wkm: first", false).unwrap();
        std::fs::write(dir.path().join("initial"), "v2").unwrap();
        git.stash_push(dir.path(), "wkm: second", false).unwrap();

        let entries = git.stash_list().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].message.contains("second"));
        assert!(entries[1].message.contains("first"));
    }

    // --- GitMutations ---

    #[test]
    fn mutations_checkout() {
        let (dir, git) = test_repo();
        git.create_branch("feature", "main").unwrap();
        git.checkout(dir.path(), "feature").unwrap();
        let branch = git.current_branch(dir.path()).unwrap();
        assert_eq!(branch, Some("feature".to_string()));
    }

    #[test]
    fn mutations_rebase_clean() {
        let (dir, git) = test_repo();
        run_git(dir.path(), &["checkout", "-b", "feature"]);
        std::fs::write(dir.path().join("feat"), "data").unwrap();
        run_git(dir.path(), &["add", "."]);
        run_git(dir.path(), &["commit", "-m", "feat"]);

        run_git(dir.path(), &["checkout", "main"]);
        std::fs::write(dir.path().join("main-file"), "data").unwrap();
        run_git(dir.path(), &["add", "."]);
        run_git(dir.path(), &["commit", "-m", "main advance"]);

        run_git(dir.path(), &["checkout", "feature"]);
        let result = git.rebase(dir.path(), "main").unwrap();
        assert_eq!(result, RebaseResult::Clean);
    }

    #[test]
    fn mutations_merge_ff() {
        let (dir, git) = test_repo();
        run_git(dir.path(), &["checkout", "-b", "feature"]);
        std::fs::write(dir.path().join("feat"), "data").unwrap();
        run_git(dir.path(), &["add", "."]);
        run_git(dir.path(), &["commit", "-m", "feat"]);

        run_git(dir.path(), &["checkout", "main"]);
        let result = git.merge_ff_only(dir.path(), "feature").unwrap();
        assert_eq!(result, MergeResult::Clean);
    }

    #[test]
    fn mutations_fetch_with_remote() {
        let (dir, git) = test_repo();
        // Set up a bare remote
        let remote_dir = tempfile::tempdir().unwrap();
        run_git(remote_dir.path(), &["init", "--bare"]);
        run_git(
            dir.path(),
            &[
                "remote",
                "add",
                "origin",
                remote_dir.path().to_str().unwrap(),
            ],
        );
        run_git(dir.path(), &["push", "-u", "origin", "main"]);
        git.fetch("origin").unwrap();
    }

    #[test]
    fn mutations_reset_hard() {
        let (dir, git) = test_repo();
        let orig = run_git_output(dir.path(), &["rev-parse", "HEAD"]);
        std::fs::write(dir.path().join("new"), "data").unwrap();
        run_git(dir.path(), &["add", "."]);
        run_git(dir.path(), &["commit", "-m", "extra"]);

        git.reset_hard(dir.path(), &orig).unwrap();
        let current = run_git_output(dir.path(), &["rev-parse", "HEAD"]);
        assert_eq!(current, orig);
    }
}
