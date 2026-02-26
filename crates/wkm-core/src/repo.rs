use std::path::{Path, PathBuf};

use crate::encoding;
use crate::error::WkmError;
use crate::git::GitDiscovery;

/// Resolved paths for a wkm-managed repository.
#[derive(Debug, Clone)]
pub struct RepoContext {
    /// The git common directory (e.g. `/home/user/project/.git`).
    pub git_common_dir: PathBuf,
    /// Path to the main worktree root.
    pub main_worktree: PathBuf,
    /// Path to the state file (`.git/wkm.toml`).
    pub state_path: PathBuf,
    /// Path to the lockfile (`.git/wkm.lock`).
    pub lock_path: PathBuf,
    /// Path to the storage directory (`~/.local/share/wkm/<encoded>/`).
    pub storage_dir: PathBuf,
}

impl RepoContext {
    /// Resolve all paths from any worktree in the repo.
    pub fn resolve(git: &dyn GitDiscovery) -> Result<Self, WkmError> {
        let git_common_dir = git.git_common_dir()?;
        let main_worktree = git.main_worktree_path()?;
        let state_path = git_common_dir.join("wkm.toml");
        let lock_path = git_common_dir.join("wkm.lock");

        let data_dir = dirs::data_dir()
            .ok_or_else(|| WkmError::Other("could not determine data directory".to_string()))?;

        let encoded_path = encoding::encode_path(main_worktree.to_string_lossy().as_ref());
        let storage_dir = data_dir.join("wkm").join(encoded_path);

        Ok(Self {
            git_common_dir,
            main_worktree,
            state_path,
            lock_path,
            storage_dir,
        })
    }

    /// Resolve from a specific working directory path.
    pub fn from_path(path: &Path) -> Result<Self, WkmError> {
        let git = crate::git::cli::CliGit::new(path);
        Self::resolve(&git)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wkm_sandbox::TestRepo;

    #[test]
    fn resolve_from_main_worktree() {
        let repo = TestRepo::new();
        let ctx = RepoContext::from_path(repo.path()).unwrap();
        assert_eq!(ctx.git_common_dir, repo.git_common_dir());
        assert!(ctx.state_path.ends_with("wkm.toml"));
        assert!(ctx.lock_path.ends_with("wkm.lock"));
        assert!(ctx.storage_dir.to_string_lossy().contains("wkm"));
    }

    #[test]
    fn resolve_from_linked_worktree() {
        let repo = TestRepo::new();
        let main_common = repo.git_common_dir();

        // Create a linked worktree
        wkm_sandbox::git(repo.path(), &["branch", "linked"]);
        let wt_path = repo.path().parent().unwrap().join("linked-wt");
        wkm_sandbox::git(
            repo.path(),
            &["worktree", "add", wt_path.to_str().unwrap(), "linked"],
        );

        let ctx = RepoContext::from_path(&wt_path).unwrap();
        // Should resolve to the same common dir
        assert_eq!(ctx.git_common_dir, main_common);
        assert_eq!(ctx.state_path, main_common.join("wkm.toml"));
    }
}
