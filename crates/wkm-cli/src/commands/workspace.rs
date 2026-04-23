use clap::{Args, Subcommand};
use wkm_core::ops::workspace;
use wkm_core::repo::RepoContext;

use crate::backend::with_backend;
use crate::ui::{Styles, tilde_path};

#[derive(Args)]
pub struct WorkspaceArgs {
    #[command(subcommand)]
    pub command: WorkspaceCommands,
}

#[derive(Subcommand)]
pub enum WorkspaceCommands {
    /// List registered workspace aliases.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Attach an alias to a worktree.
    Set {
        /// Alias to attach.
        alias: String,
        /// Tracked branch whose worktree should receive the alias. Defaults to
        /// the current directory.
        #[arg(short, long)]
        branch: Option<String>,
    },
    /// Rename an existing alias.
    Rename { old: String, new: String },
    /// Remove an alias. The underlying worktree is unaffected.
    Clear { alias: String },
}

pub fn run(args: &WorkspaceArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;

    with_backend!(ctx, &cwd, git => {
        match &args.command {
            WorkspaceCommands::List { json } => {
                let rows = workspace::list(&ctx, &git)?;
                if *json {
                    println!("{}", serde_json::to_string_pretty(&rows)?);
                } else if rows.is_empty() {
                    println!("No workspace aliases. Use `wkm worktree create --name <alias>` or `wkm workspace set`.");
                } else {
                    let s = Styles::new();
                    for row in &rows {
                        let branch = row
                            .current_branch
                            .as_deref()
                            .map(|b| format!(" → {b}"))
                            .unwrap_or_default();
                        let stale = if row.stale { "  [stale]" } else { "" };
                        let name = s.branch.apply_to(&row.alias);
                        println!(
                            "  {name}  {}{branch}{stale}",
                            tilde_path(&row.worktree_path)
                        );
                    }
                }
            }
            WorkspaceCommands::Set { alias, branch } => {
                match branch {
                    Some(b) => {
                        workspace::set(
                            &ctx,
                            alias,
                            workspace::WorkspaceTarget::Branch(b),
                        )?;
                    }
                    None => {
                        workspace::set(
                            &ctx,
                            alias,
                            workspace::WorkspaceTarget::Path(&cwd),
                        )?;
                    }
                }
                println!("Alias '{alias}' set.");
            }
            WorkspaceCommands::Rename { old, new } => {
                workspace::rename(&ctx, old, new)?;
                println!("Renamed '{old}' → '{new}'.");
            }
            WorkspaceCommands::Clear { alias } => {
                workspace::clear(&ctx, alias)?;
                println!("Cleared alias '{alias}'.");
            }
        }
        Ok(())
    })
}
