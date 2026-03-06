use clap::Args;
use wkm_core::git::cli::CliGit;
use wkm_core::ops::adopt;
use wkm_core::repo::RepoContext;

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
    } else {
        if args.branches.is_empty() {
            anyhow::bail!("Specify one or more branches, or use --all");
        }
        let result = adopt::adopt(&ctx, &git, &args.branches, args.parent.as_deref(), false)?;
        for b in &result.adopted {
            println!("Adopted '{b}'");
        }
    }
    Ok(())
}
