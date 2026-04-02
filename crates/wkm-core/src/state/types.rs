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
    /// Fully resolved storage directory path (e.g. `/home/user/.local/share/wkm/a1b2c3d4`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_dir: Option<PathBuf>,
    /// Legacy field — read from old TOML files during deserialization, never written.
    /// Migrated into `storage_dir` by `normalize_storage_dir()`.
    #[serde(skip_serializing, default)]
    resolved_storage_dir: Option<PathBuf>,
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
            resolved_storage_dir: None,
        }
    }

    /// Migrate legacy `resolved_storage_dir` into `storage_dir`.
    /// Old state files stored the user-provided base in `storage_dir` and the
    /// fully resolved path in `resolved_storage_dir`. After normalization,
    /// `storage_dir` holds the fully resolved path and the legacy field is cleared.
    pub(crate) fn normalize_storage_dir(&mut self) {
        if let Some(resolved) = self.resolved_storage_dir.take() {
            self.storage_dir = Some(resolved);
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
        /// jj operation ID recorded before sync, for rollback via `jj op restore`.
        /// Only set when sync runs through the jj backend.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        jj_op_id: Option<String>,
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
                jj_op_id: None,
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

    #[test]
    fn normalize_storage_dir_migrates_legacy_field() {
        // Simulate an old TOML file with both fields
        let toml_str = r#"
            base_branch = "main"
            storage_dir = "/custom/path"
            resolved_storage_dir = "/custom/path/a1b2c3d4"
        "#;
        let mut config: WkmConfig = toml::from_str(toml_str).unwrap();

        // Before normalization, storage_dir has the old user-provided value
        assert_eq!(config.storage_dir, Some(PathBuf::from("/custom/path")));
        assert_eq!(
            config.resolved_storage_dir,
            Some(PathBuf::from("/custom/path/a1b2c3d4"))
        );

        config.normalize_storage_dir();

        // After normalization, storage_dir holds the fully resolved path
        assert_eq!(
            config.storage_dir,
            Some(PathBuf::from("/custom/path/a1b2c3d4"))
        );
        assert!(config.resolved_storage_dir.is_none());

        // Re-serialization should NOT include resolved_storage_dir
        let re_serialized = toml::to_string_pretty(&config).unwrap();
        assert!(!re_serialized.contains("resolved_storage_dir"));
        assert!(re_serialized.contains("storage_dir"));
    }

    #[test]
    fn normalize_storage_dir_noop_when_no_legacy() {
        let mut config = WkmConfig::new("main");
        config.storage_dir = Some(PathBuf::from("/already/resolved"));
        config.normalize_storage_dir();
        assert_eq!(config.storage_dir, Some(PathBuf::from("/already/resolved")));
    }
}
