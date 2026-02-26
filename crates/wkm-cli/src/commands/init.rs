use clap::Args;
use wkm_core::ops::init::{self, InitOptions};
use wkm_core::repo::RepoContext;

#[derive(Args)]
pub struct InitArgs {
    /// Base branch name (default: main)
    #[arg(long, default_value = "main")]
    pub base: String,
}

pub fn run(args: &InitArgs) -> anyhow::Result<()> {
    let ctx = RepoContext::from_path(&std::env::current_dir()?)?;
    let opts = InitOptions {
        base_branch: args.base.clone(),
    };
    init::init(&ctx, &opts)?;
    println!("Initialized wkm with base branch '{}'", args.base);
    Ok(())
}
