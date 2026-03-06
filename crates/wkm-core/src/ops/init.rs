use std::path::PathBuf;

use crate::error::WkmError;
use crate::repo::RepoContext;
use crate::state;
use crate::state::types::{WkmConfig, WkmState};

/// Options for initializing wkm.
pub struct InitOptions {
    pub base_branch: String,
    pub storage_dir: Option<PathBuf>,
}

impl Default for InitOptions {
    fn default() -> Self {
        Self {
            base_branch: "main".to_string(),
            storage_dir: None,
        }
    }
}

/// Initialize wkm for the repository.
///
/// Creates the state file and storage directory. Idempotent — returns Ok
/// if already initialized with the same config.
pub fn init(ctx: &RepoContext, opts: &InitOptions) -> Result<WkmState, WkmError> {
    // Check if already initialized
    if let Some(existing) = state::read_state(&ctx.state_path)? {
        // Idempotent: if same config, return existing
        if existing.config.base_branch == opts.base_branch {
            return Ok(existing);
        }
        return Err(WkmError::AlreadyInitialized);
    }

    // Resolve the storage directory with collision handling
    let storage_dir = resolve_storage_dir(ctx, opts)?;

    let mut config = WkmConfig::new(&opts.base_branch);
    config.storage_dir = opts.storage_dir.clone();
    config.resolved_storage_dir = Some(storage_dir.clone());
    let new_state = WkmState::new(config);

    // Create storage directory
    std::fs::create_dir_all(&storage_dir)?;

    // Write state file
    state::write_state(&ctx.state_path, &new_state)?;

    Ok(new_state)
}

/// Resolve the storage directory, handling hash collisions.
///
/// If the user provided an explicit `storage_dir`, use that directly.
/// Otherwise, compute a hash-based candidate and check for collisions.
fn resolve_storage_dir(ctx: &RepoContext, opts: &InitOptions) -> Result<PathBuf, WkmError> {
    if opts.storage_dir.is_some() {
        // User override — use the ctx.storage_dir which already incorporates it
        return Ok(ctx.storage_dir.clone());
    }

    let candidate = ctx.storage_dir.clone();

    // If directory doesn't exist or is empty, use it
    if !candidate.exists() || is_dir_empty(&candidate) {
        return Ok(candidate);
    }

    // Directory exists and is non-empty — collision. Try suffixed variants.
    for i in 1..=100 {
        let suffixed = candidate.with_file_name(format!(
            "{}_{}",
            candidate.file_name().unwrap().to_string_lossy(),
            i
        ));
        if !suffixed.exists() || is_dir_empty(&suffixed) {
            return Ok(suffixed);
        }
    }

    Err(WkmError::Other(
        "storage directory collision: exhausted 100 suffix attempts".to_string(),
    ))
}

fn is_dir_empty(path: &std::path::Path) -> bool {
    std::fs::read_dir(path)
        .map(|mut d| d.next().is_none())
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wkm_sandbox::TestRepo;

    fn init_repo(repo: &TestRepo) -> (RepoContext, WkmState) {
        let ctx = RepoContext::from_path(repo.path()).unwrap();
        let state = init(&ctx, &InitOptions::default()).unwrap();
        (ctx, state)
    }

    #[test]
    fn init_creates_state_file() {
        let repo = TestRepo::new();
        let (ctx, state) = init_repo(&repo);
        assert!(ctx.state_path.exists());
        assert_eq!(state.version, 1);
        assert_eq!(state.config.base_branch, "main");
        assert!(state.branches.is_empty());
    }

    #[test]
    fn init_creates_storage_dir() {
        let repo = TestRepo::new();
        let (ctx, _) = init_repo(&repo);
        assert!(ctx.storage_dir.exists());
    }

    #[test]
    fn init_idempotent() {
        let repo = TestRepo::new();
        let (ctx, state1) = init_repo(&repo);
        let state2 = init(&ctx, &InitOptions::default()).unwrap();
        assert_eq!(state1.version, state2.version);
        assert_eq!(state1.config.base_branch, state2.config.base_branch);
    }

    #[test]
    fn init_custom_base() {
        let repo = TestRepo::new();
        // Create develop branch first
        repo.create_branch("develop");
        let ctx = RepoContext::from_path(repo.path()).unwrap();
        let opts = InitOptions {
            base_branch: "develop".to_string(),
            storage_dir: None,
        };
        let state = init(&ctx, &opts).unwrap();
        assert_eq!(state.config.base_branch, "develop");
    }

    #[test]
    fn init_from_linked_worktree() {
        let repo = TestRepo::new();
        let main_common = repo.git_common_dir();

        // Create a linked worktree
        repo.create_branch("linked");
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("linked-init-wt");
        wkm_sandbox::git(
            repo.path(),
            &["worktree", "add", wt_path.to_str().unwrap(), "linked"],
        );

        let ctx = RepoContext::from_path(&wt_path).unwrap();
        init(&ctx, &InitOptions::default()).unwrap();

        // State should be in main worktree's .git/
        assert_eq!(ctx.state_path, main_common.join("wkm.toml"));
        assert!(ctx.state_path.exists());

        // Cleanup
        wkm_sandbox::git(
            repo.path(),
            &["worktree", "remove", wt_path.to_str().unwrap()],
        );
    }

    #[test]
    fn init_not_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        let result = RepoContext::from_path(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn init_persists_resolved_storage_dir() {
        let repo = TestRepo::new();
        let (ctx, state) = init_repo(&repo);
        assert!(state.config.resolved_storage_dir.is_some());
        let resolved = state.config.resolved_storage_dir.unwrap();
        assert_eq!(resolved, ctx.storage_dir);

        // Re-resolve from state — should use persisted path
        let ctx2 = RepoContext::from_path(repo.path()).unwrap();
        assert_eq!(ctx2.storage_dir, resolved);
    }

    #[test]
    fn init_storage_dir_uses_hash() {
        let repo = TestRepo::new();
        let (ctx, _) = init_repo(&repo);

        // Storage dir leaf should be 8 hex chars
        let leaf = ctx.storage_dir.file_name().unwrap().to_string_lossy();
        assert_eq!(leaf.len(), 8);
        assert!(leaf.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn init_collision_increments_suffix() {
        let repo = TestRepo::new();
        let ctx = RepoContext::from_path(repo.path()).unwrap();

        // Pre-create the candidate directory with some content to simulate collision
        std::fs::create_dir_all(&ctx.storage_dir).unwrap();
        std::fs::write(ctx.storage_dir.join("marker"), "other repo").unwrap();

        let state = init(&ctx, &InitOptions::default()).unwrap();
        let resolved = state.config.resolved_storage_dir.unwrap();

        // Should have a _1 suffix
        let leaf = resolved.file_name().unwrap().to_string_lossy();
        assert!(leaf.ends_with("_1"), "expected _1 suffix, got: {leaf}");
    }
}
