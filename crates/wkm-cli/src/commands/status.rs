use clap::Args;
use wkm_core::ops::status;
use wkm_core::repo::RepoContext;

use crate::backend::with_backend;
use crate::ui::Styles;

#[derive(Args)]
pub struct StatusArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: &StatusArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;

    with_backend!(ctx, &cwd, git => {
        let s = status::status(&ctx, &git, &cwd)?;

        if args.json {
            println!("{}", serde_json::to_string_pretty(&s)?);
            return Ok(());
        }

        let styles = Styles::new();
        println!("Branch: {}", styles.branch.apply_to(&s.branch));
        if let Some(ref parent) = s.parent {
            print!("Parent: {}", styles.branch.apply_to(parent));
            if let (Some(a), Some(b)) = (s.ahead_of_parent, s.behind_parent) {
                print!(
                    " ({} {})",
                    styles.ahead.apply_to(format!("↑{a}")),
                    styles.behind.apply_to(format!("↓{b}"))
                );
            }
            println!();
        }
        if let (Some(a), Some(b)) = (s.ahead_of_remote, s.behind_remote) {
            println!(
                "Remote: {} {}",
                styles.ahead.apply_to(format!("↑{a}")),
                styles.behind.apply_to(format!("↓{b}"))
            );
        }
        if s.is_dirty {
            println!("Working tree: {}", styles.dirty.apply_to("dirty"));
        }
        if let Some(ref op) = s.in_progress_op {
            println!("In progress: {}", styles.in_progress.apply_to(op));
        }
        Ok(())
    })
}
