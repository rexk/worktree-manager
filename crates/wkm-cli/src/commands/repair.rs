use clap::Args;
use wkm_core::git::cli::CliGit;
use wkm_core::ops::repair;
use wkm_core::repo::RepoContext;

#[derive(Args)]
pub struct RepairArgs {}

pub fn run(_args: &RepairArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;
    let git = CliGit::new(&cwd);

    let result = repair::repair(&ctx, &git)?;

    if result.stale_lock_removed {
        println!("Removed stale lockfile.");
    }
    if result.wal_cleared {
        println!("Cleared incomplete operation (WAL).");
    }
    for branch in &result.branches_removed {
        println!("Removed stale state entry for '{branch}'.");
    }
    for branch in &result.worktree_paths_cleared {
        println!("Cleared missing worktree path for '{branch}'.");
    }
    for branch in &result.orphan_branches_deleted {
        println!("Deleted orphaned branch '{branch}'.");
    }

    let any_work = result.stale_lock_removed
        || result.wal_cleared
        || !result.branches_removed.is_empty()
        || !result.worktree_paths_cleared.is_empty()
        || !result.orphan_branches_deleted.is_empty();

    if !any_work {
        println!("Nothing to repair.");
    }

    Ok(())
}
