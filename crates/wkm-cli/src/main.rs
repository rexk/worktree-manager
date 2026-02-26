mod commands;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "wkm", about = "Git worktree manager")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize wkm for the current repository
    Init(commands::init::InitArgs),
    /// List tracked branches
    #[command(alias = "ls")]
    List(commands::list::ListArgs),
    /// Show status of current branch
    Status(commands::status::StatusArgs),
    /// Show branch graph
    Graph(commands::graph::GraphArgs),
    /// Print worktree path for a branch
    Cd(commands::cd::CdArgs),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Commands::Init(args) => commands::init::run(args),
        Commands::List(args) => commands::list::run(args),
        Commands::Status(args) => commands::status::run(args),
        Commands::Graph(args) => commands::graph::run(args),
        Commands::Cd(args) => commands::cd::run(args),
    }
}
