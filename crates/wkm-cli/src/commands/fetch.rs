use clap::Args;
use wkm_core::git::cli::CliGit;
use wkm_core::ops::fetch::{self, FetchResult};
use wkm_core::repo::RepoContext;

#[derive(Args)]
pub struct FetchArgs {}

pub fn run(_args: &FetchArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;
    let git = CliGit::new(&cwd);

    let result = fetch::fetch_and_ff(&ctx, &git)?;

    let wkm_state = wkm_core::state::read_state(&ctx.state_path)?.unwrap();
    let base = &wkm_state.config.base_branch;

    match result {
        FetchResult::Updated { old_ref, new_ref } => {
            let old_short = &old_ref[..7.min(old_ref.len())];
            let new_short = &new_ref[..7.min(new_ref.len())];
            println!("Fetched origin. {base}: updated ({old_short} → {new_short}).");
        }
        FetchResult::UpToDate => {
            println!("Fetched origin. {base}: already up to date.");
        }
        FetchResult::Diverged => {
            println!(
                "Fetched origin. {base}: diverged from origin/{base} (fast-forward not possible)."
            );
        }
        FetchResult::NoUpstream => {
            println!("{base} has no upstream configured. Nothing to fetch.");
        }
    }

    Ok(())
}
