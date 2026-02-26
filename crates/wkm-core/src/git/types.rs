use std::path::PathBuf;

/// Information about a git branch.
#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub name: String,
    pub commit: String,
    pub upstream: Option<String>,
}

/// Information about a git worktree.
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub head: String,
    pub branch: Option<String>,
    pub is_bare: bool,
}

/// Result of a rebase operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebaseResult {
    /// Rebase completed successfully.
    Clean,
    /// Rebase completed as a no-op (already up to date).
    UpToDate,
    /// Rebase stopped due to conflicts.
    Conflict { conflicted_files: Vec<String> },
}

/// Result of a merge operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeResult {
    /// Merge completed successfully.
    Clean,
    /// Already up to date, no merge needed.
    UpToDate,
    /// Merge stopped due to conflicts.
    Conflict { conflicted_files: Vec<String> },
    /// Fast-forward not possible (for ff-only merges).
    NotFastForward,
}

/// A git stash entry.
#[derive(Debug, Clone)]
pub struct StashEntry {
    pub index: usize,
    pub message: String,
    pub commit: String,
}

/// An in-progress git operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InProgressOp {
    Rebase,
    Merge,
    CherryPick,
    Revert,
    Bisect,
}
