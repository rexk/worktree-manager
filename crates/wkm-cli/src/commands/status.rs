use clap::Args;
use wkm_core::git::cli::CliGit;
use wkm_core::ops::status;
use wkm_core::repo::RepoContext;

#[derive(Args)]
pub struct StatusArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: &StatusArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;
    let git = CliGit::new(&cwd);
    let s = status::status(&ctx, &git, &cwd)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(());
    }

    println!("Branch: {}", s.branch);
    if let Some(ref parent) = s.parent {
        print!("Parent: {parent}");
        if let (Some(a), Some(b)) = (s.ahead_of_parent, s.behind_parent) {
            print!(" (↑{a} ↓{b})");
        }
        println!();
    }
    if let (Some(a), Some(b)) = (s.ahead_of_remote, s.behind_remote) {
        println!("Remote: ↑{a} ↓{b}");
    }
    if s.is_dirty {
        println!("Working tree: dirty");
    }
    if let Some(ref op) = s.in_progress_op {
        println!("In progress: {op}");
    }
    Ok(())
}
