pub mod cli;
pub mod jj_cli;
pub mod types;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use types::{InProgressOp, MergeResult, RebaseResult, StashEntry, WorktreeInfo};

use crate::error::WkmError;

pub type Result<T> = std::result::Result<T, WkmError>;

/// Discover git repo structure.
pub trait GitDiscovery {
    fn git_common_dir(&self) -> Result<PathBuf>;
    fn main_worktree_path(&self) -> Result<PathBuf>;
    fn current_branch(&self, worktree: &Path) -> Result<Option<String>>;
}

/// Branch operations.
pub trait GitBranches {
    fn branch_exists(&self, name: &str) -> Result<bool>;
    fn create_branch(&self, name: &str, start_point: &str) -> Result<()>;
    fn delete_branch(&self, name: &str, force: bool) -> Result<()>;
    fn force_branch(&self, name: &str, commit: &str) -> Result<()>;
    fn branch_ref(&self, name: &str) -> Result<String>;
    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool>;
    fn ahead_behind(&self, a: &str, b: &str) -> Result<(usize, usize)>;
    fn remote_tracking_branch(&self, branch: &str) -> Result<Option<String>>;
    /// Find a unique remote-tracking ref matching `name` across all remotes.
    /// Returns `Some("<remote>/<name>")` if exactly one match exists, else `None`.
    fn resolve_dwim_remote(&self, name: &str) -> Result<Option<String>>;
    fn branch_list(&self) -> Result<Vec<String>>;
    /// Return all local branches as name → OID in a single git call.
    ///
    /// Preferred over per-branch `branch_exists` / `branch_ref` when the
    /// caller needs information about many branches at once: one subprocess
    /// instead of N.
    fn branch_refs(&self) -> Result<BTreeMap<String, String>>;
}

/// Worktree operations.
pub trait GitWorktrees {
    fn worktree_list(&self) -> Result<Vec<WorktreeInfo>>;
    fn worktree_add(&self, path: &Path, branch: &str) -> Result<()>;
    fn worktree_remove(&self, path: &Path, force: bool) -> Result<()>;
    fn worktree_repair(&self) -> Result<()>;
    fn worktree_prune(&self) -> Result<()>;
}

/// Working tree state inspection.
pub trait GitStatus {
    fn is_dirty(&self, worktree: &Path) -> Result<bool>;
    fn has_changes_for_stash(&self, worktree: &Path) -> Result<bool>;
    fn has_in_progress_operation(&self, worktree: &Path) -> Result<Option<InProgressOp>>;
}

/// Stash operations.
pub trait GitStash {
    fn stash_push(&self, worktree: &Path, message: &str, include_untracked: bool)
    -> Result<String>;
    fn stash_apply(&self, worktree: &Path, commit: &str, index: bool) -> Result<()>;
    fn stash_list(&self) -> Result<Vec<StashEntry>>;
    fn stash_drop_by_index(&self, index: usize) -> Result<()>;
}

/// Mutating git operations.
pub trait GitMutations {
    fn checkout(&self, worktree: &Path, branch: &str) -> Result<()>;
    fn checkout_new_branch(&self, worktree: &Path, name: &str) -> Result<()>;
    fn rebase(&self, worktree: &Path, onto: &str) -> Result<RebaseResult>;
    fn rebase_continue(&self, worktree: &Path) -> Result<RebaseResult>;
    fn rebase_abort(&self, worktree: &Path) -> Result<()>;
    fn merge_ff_only(&self, worktree: &Path, branch: &str) -> Result<MergeResult>;
    fn merge_no_ff(&self, worktree: &Path, branch: &str, msg: &str) -> Result<MergeResult>;
    fn merge_squash(&self, worktree: &Path, branch: &str) -> Result<MergeResult>;
    fn fetch(&self, remote: &str) -> Result<()>;
    fn reset_hard(&self, worktree: &Path, commit: &str) -> Result<()>;
}
