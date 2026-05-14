use clap::{Args, Subcommand};
use wkm_core::ops::alias;
use wkm_core::repo::RepoContext;

use crate::backend::with_backend;
use crate::ui::{Styles, tilde_path};

#[derive(Args)]
pub struct AliasArgs {
    #[command(subcommand)]
    pub command: AliasCommands,
}

#[derive(Subcommand)]
pub enum AliasCommands {
    /// List registered aliases.
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

pub fn run(args: &AliasArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;

    with_backend!(ctx, &cwd, git => {
        match &args.command {
            AliasCommands::List { json } => {
                let rows = alias::list(&ctx, &git)?;
                if *json {
                    println!("{}", serde_json::to_string_pretty(&rows)?);
                } else if rows.is_empty() {
                    println!("No aliases registered. Use `wkm worktree create --name <alias>` or `wkm alias set`.");
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
            AliasCommands::Set { alias: name, branch } => {
                match branch {
                    Some(b) => {
                        alias::set(
                            &ctx,
                            &git,
                            name,
                            alias::AliasTarget::Branch(b),
                        )?;
                    }
                    None => {
                        alias::set(
                            &ctx,
                            &git,
                            name,
                            alias::AliasTarget::Path(&cwd),
                        )?;
                    }
                }
                println!("Alias '{name}' set.");
            }
            AliasCommands::Rename { old, new } => {
                alias::rename(&ctx, old, new)?;
                println!("Renamed '{old}' → '{new}'.");
            }
            AliasCommands::Clear { alias: name } => {
                alias::clear(&ctx, name)?;
                println!("Cleared alias '{name}'.");
            }
        }
        Ok(())
    })
}
