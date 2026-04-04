use std::path::PathBuf;

use clap::Args;
use wkm_core::ops::init::{self, InitOptions};
use wkm_core::repo::RepoContext;
use wkm_core::state::types::WorktreeBackend;

#[derive(Args)]
pub struct InitArgs {
    /// Base branch name (default: main)
    #[arg(long, default_value = "main")]
    pub base: String,

    /// Override storage directory for worktrees (absolute path)
    #[arg(long)]
    pub storage_dir: Option<PathBuf>,

    /// Worktree backend: git (default), git-jj (dual, default for colocated), or jj (jj-only)
    #[arg(long, value_parser = parse_worktree_backend)]
    pub worktree_backend: Option<WorktreeBackend>,
}

fn parse_worktree_backend(s: &str) -> Result<WorktreeBackend, String> {
    match s {
        "git" => Ok(WorktreeBackend::Git),
        "git-jj" | "git_jj" | "gitjj" => Ok(WorktreeBackend::GitJj),
        "jj" => Ok(WorktreeBackend::Jj),
        _ => Err(format!(
            "invalid worktree backend: '{s}' (expected: git, git-jj, or jj)"
        )),
    }
}

pub fn run(args: &InitArgs) -> anyhow::Result<()> {
    let ctx = RepoContext::from_path(&std::env::current_dir()?)?;
    let opts = InitOptions {
        base_branch: args.base.clone(),
        storage_dir: args.storage_dir.clone(),
        worktree_backend: args.worktree_backend,
    };
    let state = init::init(&ctx, &opts)?;
    let backend_str = match state.config.worktree_backend {
        WorktreeBackend::Git => "git",
        WorktreeBackend::GitJj => "git-jj (dual registration)",
        WorktreeBackend::Jj => "jj",
    };
    println!(
        "Initialized wkm with base branch '{}', worktree backend: {}",
        args.base, backend_str
    );
    Ok(())
}
