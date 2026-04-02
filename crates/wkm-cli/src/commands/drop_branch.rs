use clap::Args;
use wkm_core::git::{GitBranches, GitDiscovery, GitStatus};
use wkm_core::ops::{drop_branch, list};
use wkm_core::repo::RepoContext;

use crate::backend::with_backend;
use crate::ui;

#[derive(Args)]
pub struct DropArgs {
    /// Branch to drop from wkm tracking
    pub branch: Option<String>,
    /// Also delete the git branch
    #[arg(short = 'D', long)]
    pub delete: bool,
    /// Skip confirmation prompt for --delete
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,
}

pub fn run(args: &DropArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;

    with_backend!(ctx, &cwd, git => {
        let branch = match &args.branch {
            Some(b) => b.clone(),
            None => pick_branch(&ctx, &git)?,
        };

        if args.delete && !args.yes {
            if !ui::is_interactive() {
                anyhow::bail!(
                    "Refusing to delete git branch '{branch}' in non-interactive mode. Use --yes to confirm."
                );
            }
            let confirmed = dialoguer::Confirm::new()
                .with_prompt(format!(
                    "Delete git branch '{branch}'? This is irreversible"
                ))
                .default(false)
                .interact()?;
            if !confirmed {
                anyhow::bail!("Cancelled");
            }
        }

        let reparented = drop_branch::drop(&ctx, &git, &branch, args.delete)?;
        if !reparented.is_empty() {
            println!("Re-parented: {}", reparented.join(", "));
        }
        println!("Dropped '{branch}'");
        if args.delete {
            println!("Deleted git branch '{branch}'");
        }
        Ok(())
    })
}

fn pick_branch(
    ctx: &RepoContext,
    git: &(impl GitDiscovery + GitBranches + GitStatus),
) -> anyhow::Result<String> {
    if !ui::is_interactive() {
        anyhow::bail!("Branch argument required in non-interactive mode");
    }

    let entries = list::list(ctx, git)?;
    if entries.is_empty() {
        anyhow::bail!("No tracked branches to drop");
    }

    let items: Vec<String> = entries
        .iter()
        .map(|e| {
            let suffix = e
                .parent
                .as_deref()
                .map(|p| format!("  (parent: {p})"))
                .unwrap_or_default();
            format!("{}{suffix}", e.name)
        })
        .collect();

    let selection = dialoguer::FuzzySelect::new()
        .with_prompt("Drop branch from tracking")
        .items(&items)
        .default(0)
        .interact_opt()?;

    match selection {
        Some(idx) => Ok(entries[idx].name.clone()),
        None => anyhow::bail!("Cancelled"),
    }
}
