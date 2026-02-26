use clap::Args;
use wkm_core::ops::list;
use wkm_core::repo::RepoContext;

#[derive(Args)]
pub struct CdArgs {
    /// Branch name
    pub branch: String,
}

pub fn run(args: &CdArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;
    let path = list::cd_path(&ctx, &args.branch)?;
    println!("{}", path.display());
    Ok(())
}
