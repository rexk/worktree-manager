use clap::{Args, Subcommand};
use wkm_core::repo::RepoContext;
use wkm_core::state;

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommands,
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Get a config value
    Get {
        /// Config key (base_branch, merge_strategy, naming_strategy, prefix, max_branch_length)
        key: String,
    },
    /// Set a config value
    Set {
        /// Config key
        key: String,
        /// Config value
        value: String,
    },
    /// List all config values
    List,
}

pub fn run(args: &ConfigArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let ctx = RepoContext::from_path(&cwd)?;

    match &args.command {
        ConfigCommands::Get { key } => {
            let wkm_state = state::read_state(&ctx.state_path)?
                .ok_or_else(|| anyhow::anyhow!("wkm is not initialized"))?;
            let value = match key.as_str() {
                "base_branch" => wkm_state.config.base_branch.clone(),
                "merge_strategy" => format!("{:?}", wkm_state.config.merge_strategy),
                "naming_strategy" => format!("{:?}", wkm_state.config.naming_strategy),
                "prefix" => wkm_state
                    .config
                    .prefix
                    .clone()
                    .unwrap_or_else(|| "(unset)".to_string()),
                "max_branch_length" => wkm_state
                    .config
                    .max_branch_length
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "(unset)".to_string()),
                _ => anyhow::bail!("unknown config key: {key}"),
            };
            println!("{value}");
        }
        ConfigCommands::List => {
            let wkm_state = state::read_state(&ctx.state_path)?
                .ok_or_else(|| anyhow::anyhow!("wkm is not initialized"))?;
            let cfg = &wkm_state.config;
            println!("base_branch={}", cfg.base_branch);
            println!("merge_strategy={:?}", cfg.merge_strategy);
            println!("naming_strategy={:?}", cfg.naming_strategy);
            println!("prefix={}", cfg.prefix.as_deref().unwrap_or("(unset)"));
            println!(
                "max_branch_length={}",
                cfg.max_branch_length
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "(unset)".to_string())
            );
        }
        ConfigCommands::Set { key, value } => {
            let mut wkm_state = state::read_state(&ctx.state_path)?
                .ok_or_else(|| anyhow::anyhow!("wkm is not initialized"))?;
            match key.as_str() {
                "base_branch" => wkm_state.config.base_branch = value.clone(),
                "merge_strategy" => {
                    wkm_state.config.merge_strategy = match value.as_str() {
                        "ff" => wkm_core::state::types::MergeStrategy::Ff,
                        "merge_commit" => wkm_core::state::types::MergeStrategy::MergeCommit,
                        "squash" => wkm_core::state::types::MergeStrategy::Squash,
                        _ => anyhow::bail!(
                            "invalid merge_strategy: {value}. Use ff, merge_commit, or squash"
                        ),
                    };
                }
                "naming_strategy" => {
                    wkm_state.config.naming_strategy = match value.as_str() {
                        "timestamp" => wkm_core::state::types::NamingStrategy::Timestamp,
                        "random" => wkm_core::state::types::NamingStrategy::Random,
                        _ => anyhow::bail!(
                            "invalid naming_strategy: {value}. Use timestamp or random"
                        ),
                    };
                }
                "prefix" => {
                    wkm_state.config.prefix = if value == "unset" || value.is_empty() {
                        None
                    } else {
                        Some(value.clone())
                    };
                }
                "max_branch_length" => {
                    wkm_state.config.max_branch_length =
                        if value == "unset" || value.is_empty() {
                            None
                        } else {
                            Some(value.parse().map_err(|_| {
                                anyhow::anyhow!("invalid max_branch_length: {value}")
                            })?)
                        };
                }
                _ => anyhow::bail!("unknown config key: {key}"),
            }
            state::write_state(&ctx.state_path, &wkm_state)?;
            println!("Set {key} = {value}");
        }
    }
    Ok(())
}
