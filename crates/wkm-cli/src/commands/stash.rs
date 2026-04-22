use clap::{Args, Subcommand};
use wkm_core::ops::stash;
use wkm_core::repo::RepoContext;

use crate::backend::with_backend;

#[derive(Args)]
pub struct StashArgs {
    #[command(subcommand)]
    pub command: StashCommands,
}

#[derive(Subcommand)]
pub enum StashCommands {
    /// List all wkm stashes
    List {
        /// Filter by branch name
        #[arg(short, long)]
        branch: Option<String>,
    },
    /// Apply a branch's stash
    Apply {
        /// Branch whose stash to apply
        branch: Option<String>,
        /// Apply the stash of the branch currently in the named workspace
        #[arg(short = 'w', long = "workspace", conflicts_with = "branch")]
        workspace: Option<String>,
    },
    /// Drop a branch's stash from state
    Drop {
        /// Branch whose stash to drop
        branch: Option<String>,
        /// Drop the stash of the branch currently in the named workspace
        #[arg(short = 'w', long = "workspace", conflicts_with = "branch")]
        workspace: Option<String>,
    },
}

pub fn run(args: &StashArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;

    with_backend!(ctx, &cwd, git => {
        match &args.command {
            StashCommands::List { branch } => {
                let entries = stash::list(&ctx, branch.as_deref())?;
                if entries.is_empty() {
                    println!("No stashes.");
                } else {
                    for e in &entries {
                        println!("{}: {}", e.branch, e.commit);
                    }
                }
            }
            StashCommands::Apply { branch, workspace } => {
                let branch = resolve_branch(&ctx, &git, branch.as_deref(), workspace.as_deref())?;
                stash::apply(&ctx, &git, &branch, &cwd)?;
                println!("Applied stash for '{branch}'.");
            }
            StashCommands::Drop { branch, workspace } => {
                let branch = resolve_branch(&ctx, &git, branch.as_deref(), workspace.as_deref())?;
                stash::drop(&ctx, &branch)?;
                println!("Dropped stash for '{branch}'.");
            }
        }
        Ok(())
    })
}

fn resolve_branch(
    ctx: &wkm_core::repo::RepoContext,
    git: &impl wkm_core::git::GitDiscovery,
    branch: Option<&str>,
    workspace: Option<&str>,
) -> anyhow::Result<String> {
    match (branch, workspace) {
        (Some(b), _) => Ok(b.to_string()),
        (None, Some(alias)) => Ok(wkm_core::ops::list::branch_for_workspace(ctx, git, alias)?),
        (None, None) => anyhow::bail!("Specify a branch or use -w <workspace>"),
    }
}
