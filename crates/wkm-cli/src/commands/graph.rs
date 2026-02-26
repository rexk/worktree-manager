use clap::Args;
use wkm_core::ops::visibility;
use wkm_core::repo::RepoContext;

#[derive(Args)]
pub struct GraphArgs {}

pub fn run(_args: &GraphArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;
    let tree = visibility::render_graph(&ctx, &|_| None)?;
    println!("{tree}");
    Ok(())
}
