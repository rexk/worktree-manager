use clap::Args;
use wkm_core::git::cli::CliGit;
use wkm_core::ops::checkout;
use wkm_core::repo::RepoContext;

#[derive(Args)]
pub struct CheckoutArgs {
    /// Branch to checkout
    pub branch: String,
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
    let git = CliGit::new(&cwd);

    if args.create {
        checkout::checkout_create(&ctx, &git, &cwd, &args.branch, None)?;
        println!("Created and switched to '{}'", args.branch);
    } else {
        checkout::checkout(&ctx, &git, &cwd, &args.branch, args.include_untracked)?;
        println!("Switched to '{}'", args.branch);
    }
    Ok(())
}
