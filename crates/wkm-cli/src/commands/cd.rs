use clap::Args;
use wkm_core::ops::list;
use wkm_core::repo::RepoContext;

#[derive(Args)]
pub struct CdArgs {
    /// Branch name
    pub branch: String,
}

pub fn run(args: &CdArgs, hint: bool) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;
    let path = list::cd_path(&ctx, &args.branch)?;
    println!("{}", path.display());
    if hint && std::env::var("WKM_SHELL_SETUP").is_err() {
        eprintln!(
            "hint: run 'eval \"$(wkm shell-setup)\"' or use 'cd \"$(wkm worktree-path {})\"'",
            args.branch
        );
    }
    Ok(())
}
