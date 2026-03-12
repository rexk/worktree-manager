use clap::Args;
use console::Style;
use wkm_core::git::cli::CliGit;
use wkm_core::ops::visibility;
use wkm_core::repo::RepoContext;
use wkm_core::state;

use crate::ui::{tilde_path, Styles};

#[derive(Args)]
pub struct GraphArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Show full worktree paths instead of compact (wt) indicator
    #[arg(long, short)]
    pub long: bool,
}

pub fn run(args: &GraphArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;

    if args.json {
        let graph = visibility::graph_data(&ctx)?;
        println!("{}", serde_json::to_string_pretty(&graph)?);
    } else {
        let git = CliGit::new(&cwd);
        let current = wkm_core::git::GitDiscovery::current_branch(&git, &cwd)
            .ok()
            .flatten();

        let wkm_state = state::read_state(&ctx.state_path)?.ok_or_else(|| {
            anyhow::anyhow!("not initialized — run `wkm init` first")
        })?;
        let base_branch = wkm_state.config.base_branch.clone();
        let main_wt = ctx.main_worktree.clone();

        let annotate = move |name: &str| -> Option<String> {
            if args.long {
                if name == base_branch {
                    Some(tilde_path(&main_wt))
                } else {
                    wkm_state
                        .branches
                        .get(name)
                        .and_then(|e| e.worktree_path.as_ref())
                        .map(|p| tilde_path(p))
                }
            } else if name == base_branch {
                Some("wt".to_string())
            } else {
                wkm_state
                    .branches
                    .get(name)
                    .and_then(|e| e.worktree_path.as_ref())
                    .map(|_| "wt".to_string())
            }
        };

        let s = Styles::new();
        let tree = visibility::render_graph(&ctx, &annotate)?;

        // Colorize line by line: dim tree connectors, bold branch names,
        // green+bold for the current branch, dim annotation suffix.
        let current_style = Style::new().green().bold();
        let dim = Style::new().dim();
        for line in tree.lines() {
            // Find where the branch name starts: after tree drawing chars
            let name_start = line
                .find(|c: char| c.is_alphanumeric() || c == '_' || c == '-')
                .unwrap_or(0);
            let (prefix, rest) = line.split_at(name_start);
            // The branch name extends until a space or end of line
            let name_end = rest.find(' ').unwrap_or(rest.len());
            let (name, suffix) = rest.split_at(name_end);

            let styled_prefix = s.tree_line.apply_to(prefix);
            let styled_name = if current.as_deref() == Some(name) {
                current_style.apply_to(name).to_string()
            } else {
                s.branch.apply_to(name).to_string()
            };
            let styled_suffix = dim.apply_to(suffix);
            println!("{styled_prefix}{styled_name}{styled_suffix}");
        }
    }
    Ok(())
}
