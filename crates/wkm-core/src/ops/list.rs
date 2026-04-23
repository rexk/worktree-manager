use std::path::PathBuf;

use rayon::prelude::*;
use serde::Serialize;

use crate::error::WkmError;
use crate::git::{GitBranches, GitDiscovery};
use crate::repo::RepoContext;
use crate::state;

/// A branch entry in the list output.
#[derive(Debug, Clone, Serialize)]
pub struct ListEntry {
    pub name: String,
    pub parent: Option<String>,
    pub worktree_path: Option<PathBuf>,
    pub has_stash: bool,
    pub description: Option<String>,
    pub ahead_of_parent: Option<usize>,
    pub behind_parent: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_alias: Option<String>,
}

/// Reserved built-in token that always resolves to the main worktree.
pub const MAIN_WORKTREE_TOKEN: &str = "@main";

/// List all tracked branches.
pub fn list<G>(ctx: &RepoContext, git: &G) -> Result<Vec<ListEntry>, WkmError>
where
    G: GitBranches + GitDiscovery + Sync,
{
    let wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    // Enumerate all local branches in one git call so the per-branch existence
    // checks below are pure map lookups instead of N `git rev-parse` subprocesses.
    let refs = git.branch_refs()?;

    // For each tracked branch decide whether ahead/behind applies, then run
    // those `git rev-list --count` calls in parallel — each one is its own
    // subprocess and they're independent.
    let branch_pairs: Vec<(&str, Option<&str>)> = wkm_state
        .branches
        .iter()
        .map(|(name, branch)| {
            let parent_for_diff = branch
                .parent
                .as_deref()
                .filter(|parent| refs.contains_key(name.as_str()) && refs.contains_key(*parent));
            (name.as_str(), parent_for_diff)
        })
        .collect();

    let ahead_behind: Vec<(Option<usize>, Option<usize>)> = branch_pairs
        .par_iter()
        .map(|(name, parent)| match parent {
            Some(parent) => {
                let (a, b) = git.ahead_behind(name, parent)?;
                Ok((Some(a), Some(b)))
            }
            None => Ok((None, None)),
        })
        .collect::<Result<Vec<_>, WkmError>>()?;

    let entries = wkm_state
        .branches
        .iter()
        .zip(ahead_behind)
        .map(|((name, branch), (ahead, behind))| {
            let workspace_alias = branch.worktree_path.as_ref().and_then(|p| {
                wkm_state
                    .workspaces
                    .iter()
                    .find(|(_, w)| w.worktree_path == *p)
                    .map(|(alias, _)| alias.clone())
            });

            ListEntry {
                name: name.clone(),
                parent: branch.parent.clone(),
                worktree_path: branch.worktree_path.clone(),
                has_stash: branch.stash_commit.is_some(),
                description: branch.description.clone(),
                ahead_of_parent: ahead,
                behind_parent: behind,
                workspace_alias,
            }
        })
        .collect();
    Ok(entries)
}

/// Outcome of `cd_path`: the resolved path plus a hint about how it was matched.
#[derive(Debug, Clone)]
pub struct CdResolution {
    pub path: PathBuf,
    /// `Some((alias, branch_name))` when resolution chose an alias over a
    /// shadowed branch of the same name. Useful for the CLI to emit a
    /// one-line warning. Also populated when the user explicitly asked for
    /// an alias; the CLI can suppress the warning when it's non-ambiguous.
    pub alias_shadowed_branch: Option<(String, String)>,
}

/// Resolve `wkm wp` single positional argument.
///
/// Resolution order:
///   1. `arg == None` or `arg == "@main"` → main worktree.
///   2. `arg` starts with `@` but isn't `@main` → error.
///   3. `arg` is a workspace alias → that workspace's path.
///   4. `arg` is the base branch → main worktree.
///   5. `arg` matches main worktree's current branch → main worktree.
///   6. `arg` is a tracked branch with a secondary worktree → that path.
///   7. otherwise → error.
pub fn cd_path_resolve(
    ctx: &RepoContext,
    git: &impl GitDiscovery,
    arg: Option<&str>,
) -> Result<CdResolution, WkmError> {
    let wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    let arg = match arg {
        None => {
            return Ok(CdResolution {
                path: ctx.main_worktree.clone(),
                alias_shadowed_branch: None,
            });
        }
        Some(a) => a,
    };

    if arg == MAIN_WORKTREE_TOKEN {
        return Ok(CdResolution {
            path: ctx.main_worktree.clone(),
            alias_shadowed_branch: None,
        });
    }

    if let Some(stripped) = arg.strip_prefix('@') {
        return Err(WkmError::Other(format!(
            "unknown built-in token '@{stripped}'. Only '@main' is recognized"
        )));
    }

    // Alias-first.
    if let Some(entry) = wkm_state.workspaces.get(arg) {
        if !entry.worktree_path.exists() {
            return Err(WkmError::WorkspacePathMissing(
                arg.to_string(),
                entry.worktree_path.clone(),
            ));
        }
        let shadowed = wkm_state
            .branches
            .get(arg)
            .map(|_| (arg.to_string(), arg.to_string()));
        return Ok(CdResolution {
            path: entry.worktree_path.clone(),
            alias_shadowed_branch: shadowed,
        });
    }

    // Branch resolution (existing behaviour).
    let branch = arg;
    let path = cd_path_branch_inner(ctx, git, &wkm_state, branch)?;
    Ok(CdResolution {
        path,
        alias_shadowed_branch: None,
    })
}

/// Force branch resolution for `wkm wp -b <branch>`.
pub fn cd_path_branch(
    ctx: &RepoContext,
    git: &impl GitDiscovery,
    branch: &str,
) -> Result<PathBuf, WkmError> {
    let wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;
    cd_path_branch_inner(ctx, git, &wkm_state, branch)
}

/// Resolve a workspace alias only.
pub fn cd_path_workspace(ctx: &RepoContext, alias: &str) -> Result<PathBuf, WkmError> {
    if alias == MAIN_WORKTREE_TOKEN {
        return Ok(ctx.main_worktree.clone());
    }
    let wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;
    let entry = wkm_state
        .workspaces
        .get(alias)
        .ok_or_else(|| WkmError::WorkspaceNotFound(alias.to_string()))?;
    if !entry.worktree_path.exists() {
        return Err(WkmError::WorkspacePathMissing(
            alias.to_string(),
            entry.worktree_path.clone(),
        ));
    }
    Ok(entry.worktree_path.clone())
}

/// Legacy branch-only alias for `cd_path_branch`. Kept for call sites that
/// pass branch names directly.
pub fn cd_path(
    ctx: &RepoContext,
    git: &impl GitDiscovery,
    branch: &str,
) -> Result<PathBuf, WkmError> {
    cd_path_branch(ctx, git, branch)
}

fn cd_path_branch_inner(
    ctx: &RepoContext,
    git: &impl GitDiscovery,
    wkm_state: &crate::state::types::WkmState,
    branch: &str,
) -> Result<PathBuf, WkmError> {
    if branch == wkm_state.config.base_branch {
        return Ok(ctx.main_worktree.clone());
    }

    if git.current_branch(&ctx.main_worktree)?.as_deref() == Some(branch) {
        return Ok(ctx.main_worktree.clone());
    }

    let entry = wkm_state
        .branches
        .get(branch)
        .ok_or_else(|| WkmError::BranchNotTracked(branch.to_string()))?;

    let path = entry
        .worktree_path
        .clone()
        .ok_or_else(|| WkmError::NoWorktree(branch.to_string()))?;

    if !path.exists() {
        return Err(WkmError::WorktreePathMissing(branch.to_string(), path));
    }

    Ok(path)
}

/// Resolve the branch currently hosted in the workspace `alias`. Used by
/// mutating commands that accept `-w <alias>` as an alias for a branch.
pub fn branch_for_workspace(
    ctx: &RepoContext,
    git: &impl GitDiscovery,
    alias: &str,
) -> Result<String, WkmError> {
    if alias == MAIN_WORKTREE_TOKEN {
        return Err(WkmError::Other(
            "'@main' is not a valid workspace target for this command".to_string(),
        ));
    }
    let path = cd_path_workspace(ctx, alias)?;
    let branch = git
        .current_branch(&path)?
        .ok_or_else(|| WkmError::Other(format!("workspace '{alias}' is in detached HEAD state")))?;
    if branch.starts_with("_wkm/parked/") {
        return Err(WkmError::Other(format!(
            "workspace '{alias}' is parked on '{branch}' — start a new branch via `wkm checkout -b <name>` first"
        )));
    }
    Ok(branch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::cli::CliGit;
    use crate::ops::init::{self, InitOptions};
    use crate::state::types::BranchEntry;
    use wkm_sandbox::TestRepo;

    fn setup() -> (TestRepo, RepoContext, CliGit) {
        let repo = TestRepo::new();
        let ctx = RepoContext::from_path(repo.path()).unwrap();
        let git = CliGit::new(repo.path());
        init::init(&ctx, &InitOptions::default()).unwrap();
        (repo, ctx, git)
    }

    #[test]
    fn list_shows_all_tracked() {
        let (repo, ctx, git) = setup();
        repo.create_branch("feature");

        // Add to state
        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "feature".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: None,
                stash_commit: None,
                jj_workspace_name: None,
                description: Some("A feature".to_string()),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let entries = list(&ctx, &git).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "feature");
        assert_eq!(entries[0].parent, Some("main".to_string()));
    }

    #[test]
    fn list_shows_stash_pending() {
        let (_repo, ctx, git) = setup();

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "feature".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: None,
                stash_commit: Some("abc123".to_string()),
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let entries = list(&ctx, &git).unwrap();
        assert!(entries[0].has_stash);
    }

    #[test]
    fn cd_returns_path() {
        let (repo, ctx, git) = setup();

        let wt_path = repo.path().join("feature-wt");
        std::fs::create_dir_all(&wt_path).unwrap();

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "feature".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: Some(wt_path.clone()),
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let path = cd_path(&ctx, &git, "feature").unwrap();
        assert_eq!(path, wt_path);
    }

    #[test]
    fn cd_base_branch_returns_main_worktree() {
        let (_repo, ctx, git) = setup();
        let path = cd_path(&ctx, &git, "main").unwrap();
        assert_eq!(path, ctx.main_worktree);
    }

    #[test]
    fn cd_path_missing_directory_errors() {
        let (_repo, ctx, git) = setup();

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "feature".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: Some(PathBuf::from("/tmp/nonexistent-worktree-path-12345")),
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let result = cd_path(&ctx, &git, "feature");
        assert!(
            matches!(result, Err(WkmError::WorktreePathMissing(ref b, _)) if b == "feature"),
            "expected WorktreePathMissing, got: {result:?}"
        );
    }

    #[test]
    fn cd_path_with_slash_in_branch_name() {
        let (repo, ctx, git) = setup();

        let wt_path = repo.path().join("cursor-wt");
        std::fs::create_dir_all(&wt_path).unwrap();

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "cursor/build-cache-cleanup-8644".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: Some(wt_path.clone()),
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let path = cd_path(&ctx, &git, "cursor/build-cache-cleanup-8644").unwrap();
        assert_eq!(path, wt_path);
    }

    #[test]
    fn cd_path_existing_directory_succeeds() {
        let (repo, ctx, git) = setup();

        let wt_path = repo.path().join("feature-wt");
        std::fs::create_dir_all(&wt_path).unwrap();

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "feature".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: Some(wt_path.clone()),
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let path = cd_path(&ctx, &git, "feature").unwrap();
        assert_eq!(path, wt_path);
    }

    #[test]
    fn cd_no_worktree_errors() {
        let (repo, ctx, git) = setup();

        // Create and checkout a different branch so `feature` is not hosted
        // in the main worktree — otherwise the runtime fallback would match.
        repo.create_branch("other");
        wkm_sandbox::git(repo.path(), &["checkout", "other"]);

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "feature".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: None,
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let result = cd_path(&ctx, &git, "feature");
        assert!(matches!(result, Err(WkmError::NoWorktree(_))));
    }

    #[test]
    fn cd_path_returns_main_for_branch_in_main_worktree() {
        let (repo, ctx, git) = setup();

        // Create a branch and check it out in the main worktree.
        repo.create_branch("hosted-in-main");
        wkm_sandbox::git(repo.path(), &["checkout", "hosted-in-main"]);

        // Tracked without a stored worktree_path (the new invariant).
        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "hosted-in-main".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: None,
                stash_commit: None,
                jj_workspace_name: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        // cd_path infers main-worktree hosting at runtime.
        let path = cd_path(&ctx, &git, "hosted-in-main").unwrap();
        assert_eq!(path, ctx.main_worktree);
    }
}
