use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum WkmError {
    #[error("not a git repository")]
    NotAGitRepo,

    #[error("wkm is not initialized. Run `wkm init` first")]
    NotInitialized,

    #[error("wkm is already initialized")]
    AlreadyInitialized,

    #[error("branch '{0}' does not exist")]
    BranchNotFound(String),

    #[error("branch '{0}' already exists")]
    BranchAlreadyExists(String),

    #[error("branch '{0}' is already checked out in {1}")]
    BranchCheckedOut(String, PathBuf),

    #[error("branch '{0}' is not tracked by wkm")]
    BranchNotTracked(String),

    #[error("branch '{0}' is already tracked by wkm")]
    BranchAlreadyTracked(String),

    #[error("branch '{0}' is not a child of '{1}'")]
    NotAChild(String, String),

    #[error("branch '{0}' has dirty working tree")]
    DirtyWorktree(String),

    #[error("branch '{0}' has an in-progress {1}")]
    InProgressGitOp(String, String),

    #[error("branch '{0}' is not fast-forwardable. Run `wkm sync` first")]
    NotFastForward(String),

    #[error("directory collision for '{0}'. Use `wkm worktree create --name <name>` instead")]
    DirectoryCollision(String),

    #[error("cannot remove worktree from inside it. Run from a different worktree or use `cd`")]
    RemoveFromInside,

    #[error("no worktree for branch '{0}'. Use `wkm worktree create` or `wkm checkout`")]
    NoWorktree(String),

    #[error("another wkm operation is in progress (PID {0})")]
    LockHeld(u32),

    #[error("an operation is in progress. Run `--continue`, `--abort`, or `wkm repair`")]
    OperationInProgress,

    #[error("no operation in progress")]
    NoOperationInProgress,

    #[error("conflict in branch '{0}': {1}")]
    Conflict(String, String),

    #[error("stale lockfile from dead process (PID {0})")]
    StaleLock(u32),

    #[error("git error: {0}")]
    Git(String),

    #[error("state file error: {0}")]
    State(String),

    #[error("lock error: {0}")]
    Lock(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}
