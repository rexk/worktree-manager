use clap::Args;
use wkm_core::ops::list;
use wkm_core::repo::RepoContext;

use crate::backend::with_backend;
use crate::ui::{Styles, tilde_path};

#[derive(Args)]
pub struct ListArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Show full worktree paths instead of compact (wt) indicator
    #[arg(long, short)]
    pub long: bool,
}

pub fn run(args: &ListArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;

    with_backend!(ctx, &cwd, git => {
        let entries = list::list(&ctx, &git)?;

        if args.json {
            println!("{}", serde_json::to_string_pretty(&entries)?);
            return Ok(());
        }

        if entries.is_empty() {
            println!("No tracked branches. Use `wkm worktree create` or `wkm adopt` to add branches.");
            return Ok(());
        }

        let s = Styles::new();
        for entry in &entries {
            let name = s.branch.apply_to(&entry.name);
            let parent = entry
                .parent
                .as_deref()
                .map(|p| format!(" {}", s.parent.apply_to(format!("(parent: {p})"))))
                .unwrap_or_default();
            let wt = if args.long {
                entry
                    .worktree_path
                    .as_ref()
                    .map(|p| format!(" ({})", tilde_path(p)))
                    .unwrap_or_default()
            } else {
                entry
                    .worktree_path
                    .as_ref()
                    .map(|_| " (wt)".to_string())
                    .unwrap_or_default()
            };
            let stash = if entry.has_stash {
                format!(" {}", s.stash.apply_to("[stash]"))
            } else {
                String::new()
            };
            let ahead_behind = match (entry.ahead_of_parent, entry.behind_parent) {
                (Some(a), Some(b)) => format!(
                    " {}{}",
                    s.ahead.apply_to(format!("↑{a}")),
                    s.behind.apply_to(format!(" ↓{b}"))
                ),
                _ => String::new(),
            };
            println!("  {name}{parent}{wt}{stash}{ahead_behind}");
        }
        Ok(())
    })
}
