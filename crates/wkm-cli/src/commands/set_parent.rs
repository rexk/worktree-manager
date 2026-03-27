use clap::Args;
use wkm_core::git::GitDiscovery;
use wkm_core::git::cli::CliGit;
use wkm_core::ops::{set_parent, sync};
use wkm_core::repo::RepoContext;

#[derive(Args)]
pub struct SetParentArgs {
    /// New parent branch
    pub new_parent: String,
    /// Branch to reparent (defaults to current branch)
    pub branch: Option<String>,
}

pub fn run(args: &SetParentArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;
    let git = CliGit::new(&cwd);

    let branch = match &args.branch {
        Some(b) => b.clone(),
        None => {
            let current = GitDiscovery::current_branch(&git, &cwd)?;
            current
                .ok_or_else(|| anyhow::anyhow!("HEAD is detached. Specify a branch explicitly."))?
        }
    };

    let result = set_parent::set_parent(&ctx, &git, &branch, &args.new_parent)?;

    if result.old_parent.as_deref() == Some(&result.new_parent) {
        println!(
            "'{}' already has parent '{}'",
            result.branch, result.new_parent
        );
        return Ok(());
    }

    let old = result.old_parent.as_deref().unwrap_or("(none)");
    println!(
        "Reparented '{}': {} \u{2192} {}",
        result.branch, old, result.new_parent
    );

    // Run full sync to rebase onto the new parent
    println!("Syncing...");
    let sync_result = sync::sync(&ctx, &git)?;

    if !sync_result.synced.is_empty() {
        println!("Synced: {}", sync_result.synced.join(", "));
    }
    if let Some(ref conflicted) = sync_result.conflicted {
        println!("Conflict in '{conflicted}'. Resolve and run `wkm sync --continue`.");
    }
    if !sync_result.skipped.is_empty() {
        println!("Skipped: {}", sync_result.skipped.join(", "));
    }
    if sync_result.synced.is_empty() && sync_result.conflicted.is_none() {
        println!("All branches up to date.");
    }
    Ok(())
}
