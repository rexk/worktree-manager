use clap::Args;
use wkm_core::ops::sync;
use wkm_core::repo::RepoContext;

use crate::backend::with_backend;

#[derive(Args)]
pub struct SyncArgs {
    /// Continue after conflict resolution
    #[arg(long = "continue")]
    pub r#continue: bool,
    /// Abort and restore pre-sync state
    #[arg(long)]
    pub abort: bool,
}

pub fn run(args: &SyncArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;

    with_backend!(ctx, &cwd, git => {
        if args.abort {
            sync::sync_abort(&ctx, &git)?;
            println!("Sync aborted. All branches restored to pre-sync state.");
            return Ok(());
        }

        let result = if args.r#continue {
            sync::sync_continue(&ctx, &git)?
        } else {
            sync::sync(&ctx, &git)?
        };

        if !result.synced.is_empty() {
            println!("Synced: {}", result.synced.join(", "));
        }
        if let Some(ref branch) = result.conflicted {
            println!("Conflict in '{branch}'. Resolve and run `wkm sync --continue`.");
        }
        if !result.skipped.is_empty() {
            println!("Skipped: {}", result.skipped.join(", "));
        }
        if result.synced.is_empty() && result.conflicted.is_none() {
            println!("All branches up to date.");
        }
        Ok(())
    })
}
