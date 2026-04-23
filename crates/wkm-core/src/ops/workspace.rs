use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::encoding;
use crate::error::WkmError;
use crate::repo::RepoContext;
use crate::state;
use crate::state::lock::WkmLock;
use crate::state::types::WorkspaceEntry;

/// One entry in the `wkm workspace list` output.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceInfo {
    pub alias: String,
    pub worktree_path: PathBuf,
    pub description: Option<String>,
    pub created_at: String,
    /// `true` when the stored directory no longer exists on disk.
    pub stale: bool,
    /// The branch currently checked out in the workspace (if any).
    pub current_branch: Option<String>,
}

/// Where a new alias should point.
pub enum WorkspaceTarget<'a> {
    /// A tracked branch name — resolves to that branch's secondary worktree.
    Branch(&'a str),
    /// An explicit directory path (typically `cwd`).
    Path(&'a Path),
}

/// List all registered workspace aliases.
pub fn list(
    ctx: &RepoContext,
    git: &impl crate::git::GitDiscovery,
) -> Result<Vec<WorkspaceInfo>, WkmError> {
    let wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    let mut out = Vec::with_capacity(wkm_state.workspaces.len());
    for (alias, entry) in &wkm_state.workspaces {
        let stale = !entry.worktree_path.exists();
        let current_branch = if stale {
            None
        } else {
            git.current_branch(&entry.worktree_path).ok().flatten()
        };
        out.push(WorkspaceInfo {
            alias: alias.clone(),
            worktree_path: entry.worktree_path.clone(),
            description: entry.description.clone(),
            created_at: entry.created_at.clone(),
            stale,
            current_branch,
        });
    }
    Ok(out)
}

/// Create or reuse an alias pointing at the given target.
///
/// Errors if:
/// - the alias fails validation,
/// - the target is the main worktree,
/// - the resolved path isn't a known secondary worktree,
/// - another alias already points somewhere else.
pub fn set(ctx: &RepoContext, alias: &str, target: WorkspaceTarget<'_>) -> Result<(), WkmError> {
    encoding::validate_workspace_alias(alias).map_err(WkmError::InvalidWorkspaceAlias)?;

    let lock = WkmLock::acquire(&ctx.lock_path)?;
    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    if wkm_state.wal.is_some() {
        return Err(WkmError::OperationInProgress);
    }

    let path = match target {
        WorkspaceTarget::Branch(branch) => {
            let entry = wkm_state
                .branches
                .get(branch)
                .ok_or_else(|| WkmError::BranchNotTracked(branch.to_string()))?;
            entry
                .worktree_path
                .clone()
                .ok_or_else(|| WkmError::NoWorktree(branch.to_string()))?
        }
        WorkspaceTarget::Path(p) => {
            let canon = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
            if canon == ctx.main_worktree {
                return Err(WkmError::Other(
                    "cannot alias the main worktree (use '@main' to navigate there)".to_string(),
                ));
            }
            // The path must match an existing secondary worktree tracked by wkm.
            let matching = wkm_state
                .branches
                .values()
                .any(|b| b.worktree_path.as_deref() == Some(canon.as_path()));
            if !matching {
                return Err(WkmError::Other(format!(
                    "path {} is not a wkm-managed secondary worktree",
                    canon.display()
                )));
            }
            canon
        }
    };

    if !path.exists() {
        return Err(WkmError::WorkspacePathMissing(alias.to_string(), path));
    }

    // Duplicate alias pointing elsewhere? Reject.
    if let Some(existing) = wkm_state.workspaces.get(alias)
        && existing.worktree_path != path
    {
        return Err(WkmError::WorkspaceAliasExists(
            alias.to_string(),
            existing.worktree_path.clone(),
        ));
    }

    // Another alias already owns this path? Reject (one alias per worktree).
    if let Some((other_alias, _)) = wkm_state
        .workspaces
        .iter()
        .find(|(k, v)| k.as_str() != alias && v.worktree_path == path)
    {
        return Err(WkmError::Other(format!(
            "path already aliased as '{other_alias}' — clear or rename that alias first"
        )));
    }

    let now = chrono::Utc::now().to_rfc3339();
    wkm_state.workspaces.insert(
        alias.to_string(),
        WorkspaceEntry {
            worktree_path: path,
            created_at: now,
            description: None,
        },
    );
    state::write_state(&ctx.state_path, &wkm_state)?;
    drop(lock);
    Ok(())
}

/// Rename an alias. Errors if `old` is unknown or `new` is already taken.
pub fn rename(ctx: &RepoContext, old: &str, new: &str) -> Result<(), WkmError> {
    encoding::validate_workspace_alias(new).map_err(WkmError::InvalidWorkspaceAlias)?;

    let lock = WkmLock::acquire(&ctx.lock_path)?;
    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    if wkm_state.wal.is_some() {
        return Err(WkmError::OperationInProgress);
    }

    if old == new {
        return Ok(());
    }

    if wkm_state.workspaces.contains_key(new) {
        let existing = &wkm_state.workspaces[new];
        return Err(WkmError::WorkspaceAliasExists(
            new.to_string(),
            existing.worktree_path.clone(),
        ));
    }

    let entry = wkm_state
        .workspaces
        .remove(old)
        .ok_or_else(|| WkmError::WorkspaceNotFound(old.to_string()))?;
    wkm_state.workspaces.insert(new.to_string(), entry);
    state::write_state(&ctx.state_path, &wkm_state)?;
    drop(lock);
    Ok(())
}

/// Remove an alias. The underlying worktree directory is unaffected.
pub fn clear(ctx: &RepoContext, alias: &str) -> Result<(), WkmError> {
    let lock = WkmLock::acquire(&ctx.lock_path)?;
    let mut wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    if wkm_state.wal.is_some() {
        return Err(WkmError::OperationInProgress);
    }

    wkm_state
        .workspaces
        .remove(alias)
        .ok_or_else(|| WkmError::WorkspaceNotFound(alias.to_string()))?;
    state::write_state(&ctx.state_path, &wkm_state)?;
    drop(lock);
    Ok(())
}

/// Return the alias pointing at `path`, if any.
pub fn alias_for_path(wkm_state: &crate::state::types::WkmState, path: &Path) -> Option<String> {
    wkm_state
        .workspaces
        .iter()
        .find(|(_, v)| v.worktree_path == path)
        .map(|(k, _)| k.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::cli::CliGit;
    use crate::ops::init::{self, InitOptions};
    use crate::ops::worktree::{self, CreateOptions};
    use wkm_sandbox::TestRepo;

    fn setup() -> (TestRepo, RepoContext, CliGit) {
        let repo = TestRepo::new();
        let ctx = RepoContext::from_path(repo.path()).unwrap();
        let git = CliGit::new(repo.path());
        init::init(&ctx, &InitOptions::default()).unwrap();
        (repo, ctx, git)
    }

    #[test]
    fn set_by_branch_registers_alias() {
        let (_repo, ctx, git) = setup();
        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feat".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();

        set(&ctx, "specs", WorkspaceTarget::Branch("feat")).unwrap();

        let state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(state.workspaces.contains_key("specs"));
    }

    #[test]
    fn set_rejects_invalid_alias() {
        let (_repo, ctx, _git) = setup();
        let err = set(&ctx, "@main", WorkspaceTarget::Branch("anything"));
        assert!(matches!(err, Err(WkmError::InvalidWorkspaceAlias(_))));
    }

    #[test]
    fn set_rejects_main_worktree_path() {
        let (_repo, ctx, _git) = setup();
        let main = ctx.main_worktree.clone();
        let err = set(&ctx, "home", WorkspaceTarget::Path(&main));
        assert!(err.is_err());
    }

    #[test]
    fn set_rejects_duplicate_alias_elsewhere() {
        let (_repo, ctx, git) = setup();
        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feat-a".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();
        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feat-b".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();

        set(&ctx, "specs", WorkspaceTarget::Branch("feat-a")).unwrap();
        let err = set(&ctx, "specs", WorkspaceTarget::Branch("feat-b"));
        assert!(matches!(err, Err(WkmError::WorkspaceAliasExists(_, _))));
    }

    #[test]
    fn rename_moves_alias() {
        let (_repo, ctx, git) = setup();
        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feat".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();
        set(&ctx, "specs", WorkspaceTarget::Branch("feat")).unwrap();

        rename(&ctx, "specs", "scratch").unwrap();

        let state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(!state.workspaces.contains_key("specs"));
        assert!(state.workspaces.contains_key("scratch"));
    }

    #[test]
    fn rename_rejects_existing_target() {
        let (_repo, ctx, git) = setup();
        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feat-a".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();
        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feat-b".to_string(),
                base: None,
                description: None,
                name: None,
            },
        )
        .unwrap();
        set(&ctx, "a", WorkspaceTarget::Branch("feat-a")).unwrap();
        set(&ctx, "b", WorkspaceTarget::Branch("feat-b")).unwrap();

        let err = rename(&ctx, "a", "b");
        assert!(matches!(err, Err(WkmError::WorkspaceAliasExists(_, _))));
    }

    #[test]
    fn clear_removes_alias_keeps_worktree() {
        let (_repo, ctx, git) = setup();
        let created = worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feat".to_string(),
                base: None,
                description: None,
                name: Some("specs".to_string()),
            },
        )
        .unwrap();

        assert!(created.worktree_path.exists());

        clear(&ctx, "specs").unwrap();

        let state = state::read_state(&ctx.state_path).unwrap().unwrap();
        assert!(!state.workspaces.contains_key("specs"));
        assert!(created.worktree_path.exists());
    }

    #[test]
    fn clear_unknown_errors() {
        let (_repo, ctx, _git) = setup();
        let err = clear(&ctx, "missing");
        assert!(matches!(err, Err(WkmError::WorkspaceNotFound(_))));
    }

    #[test]
    fn list_marks_stale_when_path_gone() {
        let (_repo, ctx, git) = setup();
        worktree::create(
            &ctx,
            &git,
            &CreateOptions {
                branch: "feat".to_string(),
                base: None,
                description: None,
                name: Some("specs".to_string()),
            },
        )
        .unwrap();

        // Simulate the path going away out-of-band.
        let mut state = state::read_state(&ctx.state_path).unwrap().unwrap();
        state.workspaces.get_mut("specs").unwrap().worktree_path =
            PathBuf::from("/tmp/wkm-nonexistent-alias-path-99999");
        state::write_state(&ctx.state_path, &state).unwrap();

        let rows = list(&ctx, &git).unwrap();
        assert!(rows.iter().any(|w| w.alias == "specs" && w.stale));
    }
}
