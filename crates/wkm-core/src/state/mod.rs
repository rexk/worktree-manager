pub mod lock;
pub mod types;

use std::path::Path;

use types::WkmState;

use crate::error::WkmError;

/// Read the state file. Returns `None` if the file doesn't exist.
pub fn read_state(path: &Path) -> Result<Option<WkmState>, WkmError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            let mut state: WkmState =
                toml::from_str(&contents).map_err(|e| WkmError::State(e.to_string()))?;
            state.config.normalize_storage_dir();
            Ok(Some(state))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(WkmError::Io(e)),
    }
}

/// Write the state file atomically (write to tmpfile, then rename).
pub fn write_state(path: &Path, state: &WkmState) -> Result<(), WkmError> {
    let contents = toml::to_string_pretty(state).map_err(|e| WkmError::State(e.to_string()))?;

    let dir = path
        .parent()
        .ok_or_else(|| WkmError::State("state path has no parent directory".to_string()))?;

    let tmp = tempfile::NamedTempFile::new_in(dir)?;
    std::fs::write(tmp.path(), contents)?;
    tmp.persist(path)
        .map_err(|e| WkmError::State(format!("failed to persist state file: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::{WkmConfig, WkmState};

    #[test]
    fn read_nonexistent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wkm.toml");
        assert!(read_state(&path).unwrap().is_none());
    }

    #[test]
    fn write_then_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wkm.toml");
        let state = WkmState::new(WkmConfig::new("main"));
        write_state(&path, &state).unwrap();
        let loaded = read_state(&path).unwrap().unwrap();
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.config.base_branch, "main");
    }

    #[test]
    fn atomic_write_replaces_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wkm.toml");

        let state1 = WkmState::new(WkmConfig::new("main"));
        write_state(&path, &state1).unwrap();

        let state2 = WkmState::new(WkmConfig::new("develop"));
        write_state(&path, &state2).unwrap();

        let loaded = read_state(&path).unwrap().unwrap();
        assert_eq!(loaded.config.base_branch, "develop");
    }

    #[test]
    fn read_corrupt_file_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wkm.toml");
        std::fs::write(&path, "not valid toml {{{{").unwrap();
        assert!(read_state(&path).is_err());
    }
}
