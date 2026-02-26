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
    /// Manage worktrees
    #[command(alias = "wt")]
    Worktree(commands::worktree::WorktreeArgs),
    /// Adopt an existing branch
    Adopt(commands::adopt::AdoptArgs),
    /// Switch to a branch
    #[command(alias = "co")]
    Checkout(commands::checkout::CheckoutArgs),
    /// Sync branches by rebasing onto parents
    Sync(commands::sync::SyncArgs),
    /// Merge a child branch into its parent
    Merge(commands::merge::MergeArgs),
    /// Manage stashes
    Stash(commands::stash::StashArgs),
    /// Repair wkm state
    Repair(commands::repair::RepairArgs),
    /// Get or set config values
    Config(commands::config::ConfigArgs),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Commands::Init(args) => commands::init::run(args),
        Commands::List(args) => commands::list::run(args),
        Commands::Status(args) => commands::status::run(args),
        Commands::Graph(args) => commands::graph::run(args),
        Commands::Cd(args) => commands::cd::run(args),
        Commands::Worktree(args) => commands::worktree::run(args),
        Commands::Adopt(args) => commands::adopt::run(args),
        Commands::Checkout(args) => commands::checkout::run(args),
        Commands::Sync(args) => commands::sync::run(args),
        Commands::Merge(args) => commands::merge::run(args),
        Commands::Stash(args) => commands::stash::run(args),
        Commands::Repair(args) => commands::repair::run(args),
        Commands::Config(args) => commands::config::run(args),
    }
}
