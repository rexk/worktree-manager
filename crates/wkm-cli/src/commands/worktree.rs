use clap::{Args, Subcommand};
use wkm_core::git::{GitBranches, GitDiscovery, GitStatus};
use wkm_core::ops::{list, worktree};
use wkm_core::repo::RepoContext;

use crate::backend::with_backend;
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
    #[command(visible_alias = "rm")]
    Remove(RemoveArgs),
}

#[derive(Args)]
pub struct CreateArgs {
    /// Branch name
    pub branch: String,
    /// Base branch to branch from
    #[arg(short, long)]
    pub base: Option<String>,
    /// Description
    #[arg(short, long)]
    pub description: Option<String>,
    /// Workspace alias (persists across merges; usable with `wkm wp <name>`)
    #[arg(short = 'n', long)]
    pub name: Option<String>,
}

#[derive(Args)]
pub struct RemoveArgs {
    /// Branch name (defaults to current branch)
    pub branch: Option<String>,
    /// Remove the worktree identified by the named workspace alias
    #[arg(short = 'w', long = "workspace", conflicts_with = "branch")]
    pub workspace: Option<String>,
    /// Force removal even if dirty
    #[arg(short, long)]
    pub force: bool,
    /// Drop any pending auto-stash for the branch instead of erroring
    #[arg(long)]
    pub drop_stash: bool,
}

pub fn run(args: &WorktreeArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;

    with_backend!(ctx, &cwd, git => {
        match &args.command {
            WorktreeCommands::Create(create_args) => {
                let result = worktree::create(
                    &ctx,
                    &git,
                    &worktree::CreateOptions {
                        branch: create_args.branch.clone(),
                        base: create_args.base.clone(),
                        description: create_args.description.clone(),
                        name: create_args.name.clone(),
                    },
                )?;
                if result.created_branch {
                    println!("Created branch '{}'", result.branch);
                }
                println!("Worktree: {}", result.worktree_path.display());
                if let Some(ref alias) = create_args.name {
                    println!("Workspace alias: {alias}");
                }
            }
            WorktreeCommands::Remove(remove_args) => {
                let resolved_branch = if let Some(alias) = &remove_args.workspace {
                    Some(wkm_core::ops::list::branch_for_workspace(&ctx, &git, alias)?)
                } else {
                    remove_args.branch.clone()
                };
                let branch_ref = resolved_branch.as_deref();
                let result = worktree::remove(
                    &ctx,
                    &git,
                    &worktree::RemoveOptions {
                        branch: branch_ref,
                        force: remove_args.force,
                        drop_stash: remove_args.drop_stash,
                    },
                );
                match result {
                    Ok(removed) => println!("Removed worktree for '{removed}'"),
                    Err(e) if branch_ref.is_none() && ui::is_interactive() => {
                        let picked = pick_worktree_branch(&ctx, &git)?;
                        let _ = e;
                        let removed = worktree::remove(
                            &ctx,
                            &git,
                            &worktree::RemoveOptions {
                                branch: Some(&picked),
                                force: remove_args.force,
                                drop_stash: remove_args.drop_stash,
                            },
                        )?;
                        println!("Removed worktree for '{removed}'");
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Ok(())
    })
}

fn pick_worktree_branch(
    ctx: &RepoContext,
    git: &(impl GitDiscovery + GitBranches + GitStatus),
) -> anyhow::Result<String> {
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
