use std::path::{Path, PathBuf};

#[cfg(unix)]
use libc;

use crate::error::WkmError;

/// A PID-based lockfile.
///
/// The lock is acquired by creating a file containing the current PID.
/// On drop, the lockfile is deleted.
#[derive(Debug)]
pub struct WkmLock {
    path: PathBuf,
}

impl WkmLock {
    /// Try to acquire the lock. Returns an error if another live process holds it.
    pub fn acquire(path: &Path) -> Result<Self, WkmError> {
        let current_pid = std::process::id();

        match std::fs::read_to_string(path) {
            Ok(contents) => {
                let held_pid: u32 = contents
                    .trim()
                    .parse()
                    .map_err(|_| WkmError::Lock(format!("corrupt lockfile: {contents:?}")))?;

                if is_process_alive(held_pid) {
                    return Err(WkmError::LockHeld(held_pid));
                }

                // Stale lock from dead process — take it over
                std::fs::write(path, current_pid.to_string())?;
                Ok(Self {
                    path: path.to_path_buf(),
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::write(path, current_pid.to_string())?;
                Ok(Self {
                    path: path.to_path_buf(),
                })
            }
            Err(e) => Err(WkmError::Io(e)),
        }
    }

    /// Release the lock (also called on drop).
    pub fn release(self) {
        // Drop triggers cleanup
    }

    /// Check if a lockfile exists and return the PID if it does.
    pub fn check(path: &Path) -> Result<Option<u32>, WkmError> {
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                let pid: u32 = contents
                    .trim()
                    .parse()
                    .map_err(|_| WkmError::Lock(format!("corrupt lockfile: {contents:?}")))?;
                Ok(Some(pid))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(WkmError::Io(e)),
        }
    }

    /// Check if a lockfile is stale (held by a dead process).
    pub fn is_stale(path: &Path) -> Result<bool, WkmError> {
        match Self::check(path)? {
            Some(pid) => Ok(!is_process_alive(pid)),
            None => Ok(false),
        }
    }

    /// Delete a stale lockfile. Returns error if the process is still alive.
    pub fn remove_stale(path: &Path) -> Result<(), WkmError> {
        if let Some(pid) = Self::check(path)? {
            if is_process_alive(pid) {
                return Err(WkmError::LockHeld(pid));
            }
            std::fs::remove_file(path)?;
        }
        Ok(())
    }
}

impl Drop for WkmLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: kill with signal 0 checks if the process exists without sending a signal.
        // This is safe because signal 0 has no effect on the target process.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        // Fallback: shell out to tasklist (Windows) or assume alive
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_and_release() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wkm.lock");

        let lock = WkmLock::acquire(&path).unwrap();
        assert!(path.exists());

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents.trim().parse::<u32>().unwrap(), std::process::id());

        drop(lock);
        assert!(!path.exists());
    }

    #[test]
    fn concurrent_acquire_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wkm.lock");

        let _lock = WkmLock::acquire(&path).unwrap();

        // Second acquire should fail — same PID is alive
        let result = WkmLock::acquire(&path);
        assert!(result.is_err());
        match result.unwrap_err() {
            WkmError::LockHeld(pid) => assert_eq!(pid, std::process::id()),
            other => panic!("expected LockHeld, got: {other}"),
        }
    }

    #[test]
    fn stale_lock_reacquired() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wkm.lock");

        // Write a lockfile with a dead PID (PID 1 is init, but 99999999 shouldn't exist)
        std::fs::write(&path, "99999999").unwrap();

        // Should be able to acquire
        let lock = WkmLock::acquire(&path).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents.trim().parse::<u32>().unwrap(), std::process::id());
        drop(lock);
    }

    #[test]
    fn check_no_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wkm.lock");
        assert_eq!(WkmLock::check(&path).unwrap(), None);
    }

    #[test]
    fn is_stale_dead_pid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wkm.lock");
        std::fs::write(&path, "99999999").unwrap();
        assert!(WkmLock::is_stale(&path).unwrap());
    }

    #[test]
    fn is_stale_live_pid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wkm.lock");
        std::fs::write(&path, std::process::id().to_string()).unwrap();
        assert!(!WkmLock::is_stale(&path).unwrap());
        std::fs::remove_file(&path).unwrap(); // cleanup
    }

    #[test]
    fn remove_stale_dead_pid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wkm.lock");
        std::fs::write(&path, "99999999").unwrap();
        WkmLock::remove_stale(&path).unwrap();
        assert!(!path.exists());
    }
}
