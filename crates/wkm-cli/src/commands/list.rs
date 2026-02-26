use clap::Args;
use wkm_core::git::cli::CliGit;
use wkm_core::ops::list;
use wkm_core::repo::RepoContext;

#[derive(Args)]
pub struct ListArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: &ListArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;
    let git = CliGit::new(&cwd);
    let entries = list::list(&ctx, &git)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    if entries.is_empty() {
        println!("No tracked branches. Use `wkm worktree create` or `wkm adopt` to add branches.");
        return Ok(());
    }

    for entry in &entries {
        let parent = entry
            .parent
            .as_deref()
            .map(|p| format!(" (parent: {p})"))
            .unwrap_or_default();
        let wt = entry
            .worktree_path
            .as_ref()
            .map(|p| format!(" [{}]", p.display()))
            .unwrap_or_default();
        let stash = if entry.has_stash { " [stash]" } else { "" };
        let ahead_behind = match (entry.ahead_of_parent, entry.behind_parent) {
            (Some(a), Some(b)) => format!(" ↑{a} ↓{b}"),
            _ => String::new(),
        };
        println!("  {}{parent}{wt}{stash}{ahead_behind}", entry.name);
    }
    Ok(())
}
