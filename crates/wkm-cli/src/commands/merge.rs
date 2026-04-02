use clap::Args;
use wkm_core::git::GitDiscovery;
use wkm_core::ops::merge;
use wkm_core::repo::RepoContext;
use wkm_core::state::types::MergeStrategy;

use crate::backend::with_backend;
use crate::ui;

#[derive(Args)]
pub struct MergeArgs {
    /// Branch to merge (omit for --all)
    pub branch: Option<String>,
    /// Merge all children
    #[arg(long)]
    pub all: bool,
    /// Abort a merge in progress
    #[arg(long)]
    pub abort: bool,
    /// Merge strategy: ff, merge-commit, squash
    #[arg(long)]
    pub strategy: Option<String>,
}

pub fn run(args: &MergeArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;

    with_backend!(ctx, &cwd, git => {
        if args.abort {
            merge::merge_abort(&ctx, &git)?;
            println!("Merge aborted. State restored.");
            return Ok(());
        }

        let strategy = args.strategy.as_deref().map(|s| match s {
            "ff" => MergeStrategy::Ff,
            "merge-commit" => MergeStrategy::MergeCommit,
            "squash" => MergeStrategy::Squash,
            other => {
                eprintln!("Unknown strategy: {other}. Using default.");
                MergeStrategy::Ff
            }
        });

        if args.all {
            let merged = merge::merge_all(&ctx, &git, &cwd, strategy)?;
            if merged.is_empty() {
                println!("No children to merge.");
            } else {
                println!("Merged: {}", merged.join(", "));
            }
        } else {
            let branch = match &args.branch {
                Some(b) => b.clone(),
                None => pick_child(&ctx, &git, &cwd)?,
            };
            merge::merge(&ctx, &git, &cwd, &branch, strategy)?;
            println!("Merged '{branch}'");
        }
        Ok(())
    })
}

fn pick_child(
    ctx: &RepoContext,
    git: &impl GitDiscovery,
    cwd: &std::path::Path,
) -> anyhow::Result<String> {
    if !ui::is_interactive() {
        anyhow::bail!("Specify a branch or use --all");
    }

    let current = GitDiscovery::current_branch(git, cwd)?
        .ok_or_else(|| anyhow::anyhow!("Not on a branch"))?;

    let state = wkm_core::state::read_state(&ctx.state_path)?
        .ok_or_else(|| anyhow::anyhow!("Not initialized"))?;

    let children: Vec<String> = wkm_core::graph::children_of(&current, &state.branches)
        .into_iter()
        .map(|(name, _)| name.clone())
        .collect();

    if children.is_empty() {
        anyhow::bail!("No children of '{current}' to merge");
    }

    let selection = dialoguer::FuzzySelect::new()
        .with_prompt("Merge child branch")
        .items(&children)
        .default(0)
        .interact_opt()?;

    match selection {
        Some(idx) => Ok(children[idx].clone()),
        None => anyhow::bail!("Cancelled"),
    }
}
