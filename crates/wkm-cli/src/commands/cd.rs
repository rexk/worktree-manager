use clap::Args;
use wkm_core::ops::list;
use wkm_core::repo::RepoContext;

use crate::backend::with_backend;
use crate::ui;

#[derive(Args)]
pub struct CdArgs {
    /// Branch name, workspace alias, or `@main`.
    /// Omit to go to the main worktree.
    pub target: Option<String>,
    /// Force branch resolution (useful when an alias shadows a branch name).
    #[arg(short = 'b', long = "branch", conflicts_with = "workspace")]
    pub branch: bool,
    /// Force workspace-alias resolution (rarely needed; `@main` is always the main worktree).
    #[arg(short = 'w', long = "workspace", conflicts_with = "branch")]
    pub workspace: bool,
}

pub fn run(args: &CdArgs, hint: bool) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;

    let target = match &args.target {
        Some(t) => Some(t.clone()),
        None if args.branch || args.workspace => {
            anyhow::bail!("--branch/--workspace requires a name");
        }
        None if ui::is_interactive() => Some(pick_target(&ctx)?),
        None => None,
    };

    with_backend!(ctx, &cwd, git => {
        let path = if args.branch {
            list::cd_path_branch(&ctx, &git, target.as_deref().unwrap())?
        } else if args.workspace {
            list::cd_path_workspace(&ctx, target.as_deref().unwrap())?
        } else {
            let resolution = list::cd_path_resolve(&ctx, &git, target.as_deref())?;
            if let Some((alias, _)) = &resolution.alias_shadowed_branch {
                eprintln!(
                    "warning: '{alias}' is both a workspace alias and a branch; using the alias (use 'wkm wp -b {alias}' to force branch)"
                );
            }
            resolution.path
        };

        println!("{}", path.display());
        if hint && std::env::var("WKM_SHELL_SETUP").is_err() {
            let hint_target = target.as_deref().unwrap_or("@main");
            eprintln!(
                "hint: run 'eval \"$(wkm shell-setup)\"' or use 'cd \"$(wkm worktree-path {hint_target})\"'",
            );
        }
        Ok(())
    })
}

fn pick_target(ctx: &RepoContext) -> anyhow::Result<String> {
    with_backend!(ctx, &ctx.main_worktree, git => {
        let entries = list::list(ctx, &git)?;
        let state = wkm_core::state::read_state(&ctx.state_path)?
            .ok_or_else(|| anyhow::anyhow!("Not initialized"))?;
        let base = &state.config.base_branch;

        let mut items: Vec<(String, String)> = Vec::new();
        items.push((
            "@main".to_string(),
            format!("@main  [{}]", ctx.main_worktree.display()),
        ));
        for (alias, entry) in &state.workspaces {
            items.push((
                alias.clone(),
                format!("{alias}  (alias)  [{}]", entry.worktree_path.display()),
            ));
        }
        // Avoid duplicating the base branch entry — `@main` already covers it.
        for e in &entries {
            if &e.name == base {
                continue;
            }
            if let Some(ref wt) = e.worktree_path {
                items.push((
                    e.name.clone(),
                    format!("{}  [{}]", e.name, wt.display()),
                ));
            }
        }

        if items.is_empty() {
            anyhow::bail!("No branches with worktrees");
        }

        let display: Vec<&str> = items.iter().map(|(_, d)| d.as_str()).collect();
        let selection = dialoguer::FuzzySelect::new()
            .with_prompt("Switch to worktree")
            .items(&display)
            .default(0)
            .interact_opt()?;

        match selection {
            Some(idx) => Ok(items[idx].0.clone()),
            None => anyhow::bail!("Cancelled"),
        }
    })
}
