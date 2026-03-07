use clap::Args;
use wkm_core::git::cli::CliGit;
use wkm_core::ops::adopt;
use wkm_core::repo::RepoContext;

use crate::ui;

#[derive(Args)]
pub struct AdoptArgs {
    /// Branch names to adopt
    pub branches: Vec<String>,
    /// Parent branch
    #[arg(short, long)]
    pub parent: Option<String>,
    /// Adopt all untracked branches
    #[arg(long, conflicts_with = "branches")]
    pub all: bool,
}

pub fn run(args: &AdoptArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;
    let git = CliGit::new(&cwd);

    if args.all {
        let branches = adopt::discover_untracked(&ctx, &git)?;
        if branches.is_empty() {
            println!("No untracked branches to adopt.");
            return Ok(());
        }
        let result = adopt::adopt(&ctx, &git, &branches, args.parent.as_deref(), true)?;
        for b in &result.adopted {
            println!("Adopted '{b}'");
        }
        for b in &result.skipped {
            println!("Skipped '{b}' (already tracked)");
        }
    } else if args.branches.is_empty() {
        let selected = pick_untracked(&ctx, &git)?;
        if selected.is_empty() {
            println!("No branches selected.");
            return Ok(());
        }
        let result = adopt::adopt(&ctx, &git, &selected, args.parent.as_deref(), false)?;
        for b in &result.adopted {
            println!("Adopted '{b}'");
        }
    } else {
        let result = adopt::adopt(&ctx, &git, &args.branches, args.parent.as_deref(), false)?;
        for b in &result.adopted {
            println!("Adopted '{b}'");
        }
    }
    Ok(())
}

fn pick_untracked(ctx: &RepoContext, git: &CliGit) -> anyhow::Result<Vec<String>> {
    if !ui::is_interactive() {
        anyhow::bail!("Specify one or more branches, or use --all");
    }

    let untracked = adopt::discover_untracked(ctx, git)?;
    if untracked.is_empty() {
        anyhow::bail!("No untracked branches to adopt");
    }

    let selections = dialoguer::MultiSelect::new()
        .with_prompt("Select branches to adopt (space to toggle, enter to confirm)")
        .items(&untracked)
        .interact_opt()?;

    match selections {
        Some(idxs) => Ok(idxs.into_iter().map(|i| untracked[i].clone()).collect()),
        None => anyhow::bail!("Cancelled"),
    }
}
