use clap::Args;
use wkm_core::git::cli::CliGit;
use wkm_core::ops::adopt;
use wkm_core::repo::RepoContext;

#[derive(Args)]
pub struct AdoptArgs {
    /// Branch name to adopt
    pub branch: String,
    /// Parent branch
    #[arg(short, long)]
    pub parent: Option<String>,
}

pub fn run(args: &AdoptArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;
    let git = CliGit::new(&cwd);
    adopt::adopt(&ctx, &git, &args.branch, args.parent.as_deref())?;
    println!("Adopted '{}'", args.branch);
    Ok(())
}
