use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Top-level state file structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WkmState {
    pub version: u32,
    pub config: WkmConfig,
    #[serde(default)]
    pub branches: BTreeMap<String, BranchEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wal: Option<WalEntry>,
}

impl WkmState {
    pub fn new(config: WkmConfig) -> Self {
        Self {
            version: 1,
            config,
            branches: BTreeMap::new(),
            wal: None,
        }
    }
}

/// Repository-level configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WkmConfig {
    pub base_branch: String,
    #[serde(default)]
    pub merge_strategy: MergeStrategy,
    #[serde(default)]
    pub naming_strategy: NamingStrategy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_branch_length: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_dir: Option<PathBuf>,
}

impl WkmConfig {
    pub fn new(base_branch: impl Into<String>) -> Self {
        Self {
            base_branch: base_branch.into(),
            merge_strategy: MergeStrategy::default(),
            naming_strategy: NamingStrategy::default(),
            prefix: None,
            max_branch_length: None,
            storage_dir: None,
        }
    }
}

/// How to merge branches back to parent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeStrategy {
    #[default]
    Ff,
    MergeCommit,
    Squash,
}

/// How to auto-generate branch/worktree names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NamingStrategy {
    #[default]
    Timestamp,
    Random,
}

/// A tracked branch entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stash_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_branch: Option<String>,
}

/// Write-ahead log entry for crash recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_op_id: Option<String>,
    pub op: WalOp,
}

/// The specific operation being tracked in the WAL.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WalOp {
    Swap {
        source_branch: String,
        target_branch: String,
        source_worktree: PathBuf,
        target_worktree: Option<PathBuf>,
        main_stash: Option<String>,
        wt_stash: Option<String>,
        step: SwapStep,
    },
    Sync {
        pre_refs: BTreeMap<String, String>,
        completed: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        conflicted: Option<String>,
        pending: Vec<String>,
        temp_worktrees: Vec<(String, PathBuf)>,
    },
    Merge {
        child_branch: String,
        parent_ref: String,
        child_ref: String,
        descendant_parents: BTreeMap<String, String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        worktree_path: Option<PathBuf>,
    },
    MergeAll {
        children: Vec<String>,
        completed: Vec<String>,
        pending: Vec<String>,
    },
}

/// Steps in the swap operation for WAL tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwapStep {
    StashedMain,
    StashedBoth,
    FreedBranch,
    Swapped,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_roundtrip_minimal() {
        let state = WkmState::new(WkmConfig::new("main"));
        let toml_str = toml::to_string_pretty(&state).unwrap();
        let parsed: WkmState = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.config.base_branch, "main");
        assert!(parsed.branches.is_empty());
        assert!(parsed.wal.is_none());
    }

    #[test]
    fn state_roundtrip_with_branches() {
        let mut state = WkmState::new(WkmConfig::new("main"));
        state.branches.insert(
            "feature".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: Some("/tmp/wt".into()),
                stash_commit: None,
                description: Some("A feature".to_string()),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        let toml_str = toml::to_string_pretty(&state).unwrap();
        let parsed: WkmState = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.branches.len(), 1);
        assert_eq!(parsed.branches["feature"].parent, Some("main".to_string()));
    }

    #[test]
    fn state_roundtrip_with_wal_swap() {
        let mut state = WkmState::new(WkmConfig::new("main"));
        state.wal = Some(WalEntry {
            id: "test-uuid".to_string(),
            parent_op_id: None,
            op: WalOp::Swap {
                source_branch: "main".to_string(),
                target_branch: "feature".to_string(),
                source_worktree: "/tmp/main".into(),
                target_worktree: Some("/tmp/feat".into()),
                main_stash: Some("abc123".to_string()),
                wt_stash: None,
                step: SwapStep::StashedMain,
            },
        });
        let toml_str = toml::to_string_pretty(&state).unwrap();
        let parsed: WkmState = toml::from_str(&toml_str).unwrap();
        assert!(parsed.wal.is_some());
    }

    #[test]
    fn state_roundtrip_with_wal_sync() {
        let mut state = WkmState::new(WkmConfig::new("main"));
        let mut pre_refs = BTreeMap::new();
        pre_refs.insert("feature".to_string(), "abc123".to_string());
        state.wal = Some(WalEntry {
            id: "test-uuid".to_string(),
            parent_op_id: None,
            op: WalOp::Sync {
                pre_refs,
                completed: vec!["done-branch".to_string()],
                conflicted: Some("conflict-branch".to_string()),
                pending: vec!["pending-branch".to_string()],
                temp_worktrees: vec![("branch".to_string(), "/tmp/wt".into())],
            },
        });
        let toml_str = toml::to_string_pretty(&state).unwrap();
        let parsed: WkmState = toml::from_str(&toml_str).unwrap();
        assert!(parsed.wal.is_some());
    }

    #[test]
    fn merge_strategy_serde() {
        assert_eq!(serde_json::to_string(&MergeStrategy::Ff).unwrap(), "\"ff\"");
        assert_eq!(
            serde_json::to_string(&MergeStrategy::MergeCommit).unwrap(),
            "\"merge_commit\""
        );
        assert_eq!(
            serde_json::to_string(&MergeStrategy::Squash).unwrap(),
            "\"squash\""
        );
    }
}
