use std::path::{Path, PathBuf};

use crate::encoding;
use crate::error::WkmError;
use crate::git::GitDiscovery;
use crate::state;

/// Which VCS backend is available for this repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcsBackend {
    /// Pure git repository — use git CLI for all operations.
    Git,
    /// Colocated jj+git repository with `jj` CLI available on PATH.
    JjColocated,
}

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
    /// Detected VCS backend.
    pub vcs_backend: VcsBackend,
}

/// Resolve the base data directory for wkm storage using tiered resolution:
/// 1. `WKM_DATA_DIR` env var (returned directly, not under `wkm/`)
/// 2. `XDG_DATA_HOME` env var → `$XDG_DATA_HOME/wkm/`
/// 3. Fallback: `~/.local/share/wkm/`
fn resolve_base_data_dir() -> Result<PathBuf, WkmError> {
    if let Ok(dir) = std::env::var("WKM_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }

    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        return Ok(PathBuf::from(dir).join("wkm"));
    }

    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .map_err(|_| {
            WkmError::Other(
                "could not determine home directory: neither HOME nor USERPROFILE is set"
                    .to_string(),
            )
        })?;

    #[cfg(windows)]
    return Ok(home.join("AppData/Local/wkm"));

    #[cfg(not(windows))]
    Ok(home.join(".local/share/wkm"))
}

/// Detect whether the repository is a colocated jj+git repo with `jj` available.
fn detect_vcs_backend(main_worktree: &Path) -> VcsBackend {
    // Check if `.jj/` directory exists at the repo root
    if !main_worktree.join(".jj").is_dir() {
        return VcsBackend::Git;
    }
    // Check if `jj` CLI is available on PATH
    match std::process::Command::new("jj")
        .arg("version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) if status.success() => VcsBackend::JjColocated,
        _ => VcsBackend::Git,
    }
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

        // Load existing state to check for persisted storage_dir
        let existing_state = state::read_state(&state_path)?;

        let storage_dir = if let Some(ref state) = existing_state {
            if let Some(ref dir) = state.config.storage_dir {
                // Persisted fully-resolved path — use directly
                dir.clone()
            } else {
                let base_dir = resolve_base_data_dir()?;
                let hash = encoding::hash_path(main_worktree.to_string_lossy().as_ref());
                base_dir.join(hash)
            }
        } else {
            // No state yet (pre-init) — compute default
            let base_dir = resolve_base_data_dir()?;
            let hash = encoding::hash_path(main_worktree.to_string_lossy().as_ref());
            base_dir.join(hash)
        };

        let vcs_backend = detect_vcs_backend(&main_worktree);

        Ok(Self {
            git_common_dir,
            main_worktree,
            state_path,
            lock_path,
            storage_dir,
            repo_name,
            vcs_backend,
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
