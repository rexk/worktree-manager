use clap::{Args, Subcommand};
use wkm_core::git::cli::CliGit;
use wkm_core::ops::{list, worktree};
use wkm_core::repo::RepoContext;

use crate::ui;

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
            let branch = remove_args.branch.as_deref();
            // Try the operation directly first — the core defaults to
            // current branch when None. If that fails and we're interactive,
            // offer a picker.
            let result = worktree::remove(&ctx, &git, branch, remove_args.force);
            match result {
                Ok(removed) => println!("Removed worktree for '{removed}'"),
                Err(e) if branch.is_none() && ui::is_interactive() => {
                    // Current branch might not have a worktree, or we might be
                    // in the main worktree. Offer a picker.
                    let picked = pick_worktree_branch(&ctx, &git)?;
                    // Show the original error context if the picker was needed
                    // because of a specific error (e.g., NoWorktree for current branch)
                    let _ = e; // consume original error
                    let removed = worktree::remove(&ctx, &git, Some(&picked), remove_args.force)?;
                    println!("Removed worktree for '{removed}'");
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
    Ok(())
}

fn pick_worktree_branch(ctx: &RepoContext, git: &CliGit) -> anyhow::Result<String> {
    let entries = list::list(ctx, git)?;
    let with_worktrees: Vec<_> = entries
        .iter()
        .filter(|e| e.worktree_path.is_some())
        .collect();

    if with_worktrees.is_empty() {
        anyhow::bail!("No branches have worktrees to remove");
    }

    let items: Vec<String> = with_worktrees
        .iter()
        .map(|e| {
            format!(
                "{}  [{}]",
                e.name,
                e.worktree_path.as_ref().unwrap().display()
            )
        })
        .collect();

    let selection = dialoguer::FuzzySelect::new()
        .with_prompt("Remove worktree for branch")
        .items(&items)
        .default(0)
        .interact_opt()?;

    match selection {
        Some(idx) => Ok(with_worktrees[idx].name.clone()),
        None => anyhow::bail!("Cancelled"),
    }
}
