use clap::{Args, Subcommand};
use wkm_core::git::cli::CliGit;
use wkm_core::ops::worktree;
use wkm_core::repo::RepoContext;

#[derive(Args)]
pub struct WorktreeArgs {
    #[command(subcommand)]
    pub command: WorktreeCommands,
}

#[derive(Subcommand)]
pub enum WorktreeCommands {
    /// Create a new worktree
    Create(CreateArgs),
    /// Remove a worktree
    #[command(alias = "rm")]
    Remove(RemoveArgs),
}

#[derive(Args)]
pub struct CreateArgs {
    /// Branch name
    pub branch: String,
    /// Explicit directory name
    #[arg(long)]
    pub name: Option<String>,
    /// Base branch to branch from
    #[arg(short, long)]
    pub base: Option<String>,
    /// Description
    #[arg(short, long)]
    pub description: Option<String>,
}

#[derive(Args)]
pub struct RemoveArgs {
    /// Branch name (defaults to current branch)
    pub branch: Option<String>,
    /// Force removal even if dirty
    #[arg(short, long)]
    pub force: bool,
}

pub fn run(args: &WorktreeArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;
    let git = CliGit::new(&cwd);

    match &args.command {
        WorktreeCommands::Create(create_args) => {
            let result = worktree::create(
                &ctx,
                &git,
                &worktree::CreateOptions {
                    branch: create_args.branch.clone(),
                    name: create_args.name.clone(),
                    base: create_args.base.clone(),
                    description: create_args.description.clone(),
                },
            )?;
            if result.created_branch {
                println!("Created branch '{}'", result.branch);
            }
            println!("Worktree: {}", result.worktree_path.display());
        }
        WorktreeCommands::Remove(remove_args) => {
            let removed =
                worktree::remove(&ctx, &git, remove_args.branch.as_deref(), remove_args.force)?;
            println!("Removed worktree for '{removed}'");
        }
    }
    Ok(())
}
