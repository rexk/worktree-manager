use clap::Args;
use wkm_core::ops::visibility;
use wkm_core::repo::RepoContext;

#[derive(Args)]
pub struct GraphArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: &GraphArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;

    if args.json {
        let graph = visibility::graph_data(&ctx)?;
        println!("{}", serde_json::to_string_pretty(&graph)?);
    } else {
        let tree = visibility::render_graph(&ctx, &|_| None)?;
        println!("{tree}");
    }
    Ok(())
}
