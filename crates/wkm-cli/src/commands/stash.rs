use clap::{Args, Subcommand};
use wkm_core::git::cli::CliGit;
use wkm_core::ops::stash;
use wkm_core::repo::RepoContext;

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
        branch: String,
    },
    /// Drop a branch's stash from state
    Drop {
        /// Branch whose stash to drop
        branch: String,
    },
}

pub fn run(args: &StashArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;
    let git = CliGit::new(&cwd);

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
        StashCommands::Apply { branch } => {
            stash::apply(&ctx, &git, branch, &cwd)?;
            println!("Applied stash for '{branch}'.");
        }
        StashCommands::Drop { branch } => {
            stash::drop(&ctx, branch)?;
            println!("Dropped stash for '{branch}'.");
        }
    }
    Ok(())
}
