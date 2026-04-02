use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::WkmError;

use super::cli::CliGit;
use super::types::{InProgressOp, MergeResult, RebaseResult, StashEntry, WorktreeInfo};
use super::{GitBranches, GitDiscovery, GitMutations, GitStash, GitStatus, GitWorktrees, Result};

/// Backend for colocated jj+git repositories.
///
/// Wraps `CliGit` via composition: delegates all operations to git by default,
/// selectively overriding specific methods where jj provides better behavior.
pub struct JjCli {
    inner: CliGit,
    /// Working directory for running jj commands.
    work_dir: PathBuf,
}

impl JjCli {
    pub fn new(work_dir: impl Into<PathBuf>) -> Self {
        let work_dir = work_dir.into();
        Self {
            inner: CliGit::new(&work_dir),
            work_dir,
        }
    }

    /// Run a jj command in the default work_dir and return stdout, or error if it fails.
    fn jj_run_ok(&self, args: &[&str]) -> Result<String> {
        self.jj_run_ok_in(&self.work_dir, args)
    }

    /// Run a jj command in a specific directory and return stdout, or error if it fails.
    fn jj_run_ok_in(&self, dir: &Path, args: &[&str]) -> Result<String> {
        let output = Command::new("jj")
            .args(args)
            .current_dir(dir)
            .output()
            .map_err(|e| WkmError::Git(format!("failed to run jj: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(WkmError::Git(format!(
                "jj {} failed: {}",
                args.join(" "),
                stderr.trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Run a jj command and return the raw output (status + stdout + stderr).
    #[allow(dead_code)]
    fn jj_run(&self, args: &[&str]) -> Result<std::process::Output> {
        self.jj_run_in(&self.work_dir, args)
    }

    /// Run a jj command in a specific directory, returning raw output.
    fn jj_run_in(&self, dir: &Path, args: &[&str]) -> Result<std::process::Output> {
        let output = Command::new("jj")
            .args(args)
            .current_dir(dir)
            .output()
            .map_err(|e| WkmError::Git(format!("failed to run jj: {e}")))?;
        Ok(output)
    }

    /// Get the current jj operation ID for use in WAL/rollback.
    pub fn current_op_id(&self) -> Result<String> {
        self.jj_run_ok(&["op", "log", "--no-graph", "--limit=1", "-T", "self.id()"])
    }
}

// -- Trait implementations: delegate to inner CliGit by default --

impl GitDiscovery for JjCli {
    fn git_common_dir(&self) -> Result<PathBuf> {
        self.inner.git_common_dir()
    }

    fn main_worktree_path(&self) -> Result<PathBuf> {
        self.inner.main_worktree_path()
    }

    fn current_branch(&self, worktree: &Path) -> Result<Option<String>> {
        self.inner.current_branch(worktree)
    }
}

impl GitBranches for JjCli {
    fn branch_exists(&self, name: &str) -> Result<bool> {
        self.inner.branch_exists(name)
    }

    fn create_branch(&self, name: &str, start_point: &str) -> Result<()> {
        self.inner.create_branch(name, start_point)
    }

    fn delete_branch(&self, name: &str, force: bool) -> Result<()> {
        self.inner.delete_branch(name, force)
    }

    fn force_branch(&self, name: &str, commit: &str) -> Result<()> {
        self.inner.force_branch(name, commit)
    }

    fn branch_ref(&self, name: &str) -> Result<String> {
        self.inner.branch_ref(name)
    }

    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        self.inner.is_ancestor(ancestor, descendant)
    }

    fn ahead_behind(&self, a: &str, b: &str) -> Result<(usize, usize)> {
        self.inner.ahead_behind(a, b)
    }

    fn remote_tracking_branch(&self, branch: &str) -> Result<Option<String>> {
        self.inner.remote_tracking_branch(branch)
    }

    fn branch_list(&self) -> Result<Vec<String>> {
        self.inner.branch_list()
    }
}

impl GitWorktrees for JjCli {
    fn worktree_list(&self) -> Result<Vec<WorktreeInfo>> {
        self.inner.worktree_list()
    }

    fn worktree_add(&self, path: &Path, branch: &str) -> Result<()> {
        self.inner.worktree_add(path, branch)
    }

    fn worktree_remove(&self, path: &Path, force: bool) -> Result<()> {
        self.inner.worktree_remove(path, force)
    }

    fn worktree_repair(&self) -> Result<()> {
        self.inner.worktree_repair()
    }

    fn worktree_prune(&self) -> Result<()> {
        self.inner.worktree_prune()
    }
}

impl GitStatus for JjCli {
    fn is_dirty(&self, worktree: &Path) -> Result<bool> {
        self.inner.is_dirty(worktree)
    }

    fn has_changes_for_stash(&self, worktree: &Path) -> Result<bool> {
        self.inner.has_changes_for_stash(worktree)
    }

    fn has_in_progress_operation(&self, worktree: &Path) -> Result<Option<InProgressOp>> {
        self.inner.has_in_progress_operation(worktree)
    }
}

impl GitStash for JjCli {
    fn stash_push(
        &self,
        worktree: &Path,
        message: &str,
        include_untracked: bool,
    ) -> Result<String> {
        self.inner.stash_push(worktree, message, include_untracked)
    }

    fn stash_apply(&self, worktree: &Path, commit: &str, index: bool) -> Result<()> {
        self.inner.stash_apply(worktree, commit, index)
    }

    fn stash_list(&self) -> Result<Vec<StashEntry>> {
        self.inner.stash_list()
    }

    fn stash_drop_by_index(&self, index: usize) -> Result<()> {
        self.inner.stash_drop_by_index(index)
    }
}

impl GitMutations for JjCli {
    fn checkout(&self, worktree: &Path, branch: &str) -> Result<()> {
        self.inner.checkout(worktree, branch)
    }

    fn checkout_new_branch(&self, worktree: &Path, name: &str) -> Result<()> {
        self.inner.checkout_new_branch(worktree, name)
    }

    fn rebase(&self, worktree: &Path, onto: &str) -> Result<RebaseResult> {
        // Single-branch rebase delegates to git. The jj-specific cascade rebase
        // is handled at a higher level in sync/jj.rs via direct jj CLI calls,
        // because jj rebase operates on the entire repo (not per-worktree).
        self.inner.rebase(worktree, onto)
    }

    fn rebase_continue(&self, worktree: &Path) -> Result<RebaseResult> {
        self.inner.rebase_continue(worktree)
    }

    fn rebase_abort(&self, worktree: &Path) -> Result<()> {
        self.inner.rebase_abort(worktree)
    }

    fn merge_ff_only(&self, worktree: &Path, branch: &str) -> Result<MergeResult> {
        self.inner.merge_ff_only(worktree, branch)
    }

    fn merge_no_ff(&self, worktree: &Path, branch: &str, msg: &str) -> Result<MergeResult> {
        self.inner.merge_no_ff(worktree, branch, msg)
    }

    fn merge_squash(&self, worktree: &Path, branch: &str) -> Result<MergeResult> {
        self.inner.merge_squash(worktree, branch)
    }

    fn fetch(&self, remote: &str) -> Result<()> {
        self.inner.fetch(remote)
    }

    fn reset_hard(&self, worktree: &Path, commit: &str) -> Result<()> {
        self.inner.reset_hard(worktree, commit)
    }
}
