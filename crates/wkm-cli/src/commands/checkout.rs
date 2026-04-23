use clap::Args;
use wkm_core::git::{GitBranches, GitDiscovery, GitStatus};
use wkm_core::ops::{checkout, list};
use wkm_core::repo::RepoContext;

use crate::backend::with_backend;
use crate::ui;

#[derive(Args)]
pub struct CheckoutArgs {
    /// Branch to checkout
    pub branch: Option<String>,
    /// Create a new branch
    #[arg(short = 'b')]
    pub create: bool,
    /// Include untracked files in stash
    #[arg(long)]
    pub include_untracked: bool,
}

pub fn run(args: &CheckoutArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;

    with_backend!(ctx, &cwd, git => {
        let branch = match &args.branch {
            Some(b) => b.clone(),
            None => {
                if args.create {
                    anyhow::bail!("Branch name required with -b");
                }
                pick_branch(&ctx, &git)?
            }
        };

        if args.create {
            checkout::checkout_create(&ctx, &git, &cwd, &branch, None)?;
            println!("Created and switched to '{branch}'");
        } else {
            checkout::checkout(&ctx, &git, &cwd, &branch, args.include_untracked)?;
            println!("Switched to '{branch}'");
        }
        Ok(())
    })
}

fn pick_branch(
    ctx: &RepoContext,
    git: &(impl GitDiscovery + GitBranches + GitStatus + Sync),
) -> anyhow::Result<String> {
    if !ui::is_interactive() {
        anyhow::bail!("Branch argument required in non-interactive mode");
    }

    let entries = list::list(ctx, git)?;
    if entries.is_empty() {
        anyhow::bail!("No tracked branches");
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
        .with_prompt("Switch to branch")
        .items(&items)
        .default(0)
        .interact_opt()?;

    match selection {
        Some(idx) => Ok(entries[idx].name.clone()),
        None => anyhow::bail!("Cancelled"),
    }
}
