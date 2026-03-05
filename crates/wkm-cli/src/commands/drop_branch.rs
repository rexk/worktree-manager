use clap::Args;
use wkm_core::git::cli::CliGit;
use wkm_core::ops::drop_branch;
use wkm_core::repo::RepoContext;

#[derive(Args)]
pub struct DropArgs {
    /// Branch to drop from wkm tracking
    pub branch: String,
    /// Also delete the git branch
    #[arg(short = 'D', long)]
    pub delete: bool,
}

pub fn run(args: &DropArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;
    let git = CliGit::new(&cwd);
    let reparented = drop_branch::drop(&ctx, &git, &args.branch, args.delete)?;
    if !reparented.is_empty() {
        println!("Re-parented: {}", reparented.join(", "));
    }
    println!("Dropped '{}'", args.branch);
    if args.delete {
        println!("Deleted git branch '{}'", args.branch);
    }
    Ok(())
}
