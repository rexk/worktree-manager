use clap::Args;
use wkm_core::git::cli::CliGit;
use wkm_core::ops::merge;
use wkm_core::repo::RepoContext;
use wkm_core::state::types::MergeStrategy;

#[derive(Args)]
pub struct MergeArgs {
    /// Branch to merge (omit for --all)
    pub branch: Option<String>,
    /// Merge all children
    #[arg(long)]
    pub all: bool,
    /// Abort a merge in progress
    #[arg(long)]
    pub abort: bool,
    /// Merge strategy: ff, merge-commit, squash
    #[arg(long)]
    pub strategy: Option<String>,
}

pub fn run(args: &MergeArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;
    let git = CliGit::new(&cwd);

    if args.abort {
        merge::merge_abort(&ctx, &git)?;
        println!("Merge aborted. State restored.");
        return Ok(());
    }

    let strategy = args.strategy.as_deref().map(|s| match s {
        "ff" => MergeStrategy::Ff,
        "merge-commit" => MergeStrategy::MergeCommit,
        "squash" => MergeStrategy::Squash,
        other => {
            eprintln!("Unknown strategy: {other}. Using default.");
            MergeStrategy::Ff
        }
    });

    if args.all {
        let merged = merge::merge_all(&ctx, &git, &cwd, strategy)?;
        if merged.is_empty() {
            println!("No children to merge.");
        } else {
            println!("Merged: {}", merged.join(", "));
        }
    } else {
        let branch = args
            .branch
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Specify a branch or use --all"))?;
        merge::merge(&ctx, &git, &cwd, branch, strategy)?;
        println!("Merged '{branch}'");
    }
    Ok(())
}
