use clap::Args;
use wkm_core::ops::list;
use wkm_core::repo::RepoContext;

use crate::backend::with_backend;
use crate::ui;

#[derive(Args)]
pub struct CdArgs {
    /// Branch name
    pub branch: Option<String>,
}

pub fn run(args: &CdArgs, hint: bool) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;

    let branch = match &args.branch {
        Some(b) => b.clone(),
        None => pick_branch_with_worktree(&ctx)?,
    };

    let path = list::cd_path(&ctx, &branch)?;
    println!("{}", path.display());
    if hint && std::env::var("WKM_SHELL_SETUP").is_err() {
        eprintln!(
            "hint: run 'eval \"$(wkm shell-setup)\"' or use 'cd \"$(wkm worktree-path {branch})\"'",
        );
    }
    Ok(())
}

fn pick_branch_with_worktree(ctx: &RepoContext) -> anyhow::Result<String> {
    if !ui::is_interactive() {
        anyhow::bail!("Branch argument required in non-interactive mode");
    }

    with_backend!(ctx, &ctx.main_worktree, git => {
        let entries = list::list(ctx, &git)?;
        let state = wkm_core::state::read_state(&ctx.state_path)?
            .ok_or_else(|| anyhow::anyhow!("Not initialized"))?;
        let base = &state.config.base_branch;

        let mut items: Vec<(String, String)> = Vec::new();
        items.push((
            base.clone(),
            format!("{base}  [{}]", ctx.main_worktree.display()),
        ));
        for e in &entries {
            if let Some(ref wt) = e.worktree_path {
                items.push((e.name.clone(), format!("{}  [{}]", e.name, wt.display())));
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
