use std::path::{Path, PathBuf};

use crate::encoding;
use crate::error::WkmError;
use crate::git::GitDiscovery;
use crate::state;

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
    /// Path to the storage directory.
    pub storage_dir: PathBuf,
    /// Repository name (last component of main worktree path).
    pub repo_name: String,
}

/// Resolve the base data directory for wkm storage using tiered resolution:
/// 1. Per-repo config `storage_dir` (returned directly, not under `wkm/`)
/// 2. `WKM_DATA_DIR` env var (returned directly, not under `wkm/`)
/// 3. `XDG_DATA_HOME` env var → `$XDG_DATA_HOME/wkm/`
/// 4. Fallback: `~/.local/share/wkm/`
fn resolve_base_data_dir(config_storage_dir: Option<&Path>) -> Result<PathBuf, WkmError> {
    if let Some(dir) = config_storage_dir {
        return Ok(dir.to_path_buf());
    }

    if let Ok(dir) = std::env::var("WKM_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }

    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        return Ok(PathBuf::from(dir).join("wkm"));
    }

    let home = std::env::var("HOME").map(PathBuf::from).map_err(|_| {
        WkmError::Other("could not determine home directory: HOME not set".to_string())
    })?;

    Ok(home.join(".local/share/wkm"))
}

impl RepoContext {
    /// Resolve all paths from any worktree in the repo.
    pub fn resolve(git: &dyn GitDiscovery) -> Result<Self, WkmError> {
        let git_common_dir = git.git_common_dir()?;
        let main_worktree = git.main_worktree_path()?;
        let state_path = git_common_dir.join("wkm.toml");
        let lock_path = git_common_dir.join("wkm.lock");

        let repo_name = main_worktree
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".to_string());

        // Load existing state to check for resolved or per-repo config storage_dir
        let existing_state = state::read_state(&state_path)?;

        let storage_dir = if let Some(ref state) = existing_state {
            if let Some(ref resolved) = state.config.resolved_storage_dir {
                // Already resolved — use persisted path directly
                resolved.clone()
            } else if state.config.storage_dir.is_some() {
                // Per-repo override (legacy path)
                let base_dir = resolve_base_data_dir(state.config.storage_dir.as_deref())?;
                let hash = encoding::hash_path(main_worktree.to_string_lossy().as_ref());
                base_dir.join(hash)
            } else {
                let base_dir = resolve_base_data_dir(None)?;
                let hash = encoding::hash_path(main_worktree.to_string_lossy().as_ref());
                base_dir.join(hash)
            }
        } else {
            // No state yet (pre-init) — compute default
            let base_dir = resolve_base_data_dir(None)?;
            let hash = encoding::hash_path(main_worktree.to_string_lossy().as_ref());
            base_dir.join(hash)
        };

        Ok(Self {
            git_common_dir,
            main_worktree,
            state_path,
            lock_path,
            storage_dir,
            repo_name,
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
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("linked-wt");
        wkm_sandbox::git(
            repo.path(),
            &["worktree", "add", wt_path.to_str().unwrap(), "linked"],
        );

        let ctx = RepoContext::from_path(&wt_path).unwrap();
        // Should resolve to the same common dir
        assert_eq!(ctx.git_common_dir, main_common);
        assert_eq!(ctx.state_path, main_common.join("wkm.toml"));

        // Cleanup before tempdir drops
        wkm_sandbox::git(
            repo.path(),
            &["worktree", "remove", wt_path.to_str().unwrap()],
        );
    }
}
