use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::tmux;

#[derive(Debug)]
pub enum AcquireError {
    Io(io::Error),
    AlreadyRunning { pid: Option<u32> },
}

impl From<io::Error> for AcquireError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

pub struct InstanceGuard {
    lock_path: PathBuf,
    pid: u32,
}

impl InstanceGuard {
    pub fn acquire(current_pid: u32) -> Result<Self, AcquireError> {
        Self::acquire_at(Self::default_lock_path(), current_pid)
    }

    fn acquire_at(lock_path: PathBuf, current_pid: u32) -> Result<Self, AcquireError> {
        loop {
            match Self::create_lock(&lock_path, current_pid) {
                Ok(()) => {
                    return Ok(Self {
                        lock_path,
                        pid: current_pid,
                    });
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    let existing_pid = Self::read_lock_pid(&lock_path);
                    match existing_pid {
                        Some(pid) if pid != current_pid && tmux::pid_looks_like_deck(pid) =>
                        {
                            return Err(AcquireError::AlreadyRunning { pid: Some(pid) });
                        }
                        Some(pid) if pid == current_pid => {
                            return Err(AcquireError::AlreadyRunning { pid: Some(pid) });
                        }
                        _ => {
                            let _ = fs::remove_file(&lock_path);
                            continue;
                        }
                    }
                }
                Err(err) => return Err(AcquireError::Io(err)),
            }
        }
    }

    fn default_lock_path() -> PathBuf {
        PathBuf::from(format!("/tmp/{}.lock", env!("CARGO_PKG_NAME")))
    }

    fn create_lock(lock_path: &Path, current_pid: u32) -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(lock_path)?;
        writeln!(file, "{current_pid}")?;
        file.flush()?;
        Ok(())
    }

    fn read_lock_pid(lock_path: &Path) -> Option<u32> {
        let raw = fs::read_to_string(lock_path).ok()?;
        raw.trim().parse().ok()
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if Self::read_lock_pid(&self.lock_path) == Some(self.pid) {
            let _ = fs::remove_file(&self.lock_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AcquireError, InstanceGuard};
    use std::fs;
    use std::path::PathBuf;

    fn test_lock_path(name: &str) -> PathBuf {
        PathBuf::from(format!(
            "/tmp/deck-test-{name}-{}.lock",
            std::process::id()
        ))
    }

    #[test]
    fn acquires_and_releases_lock() {
        let path = test_lock_path("acquire-release");
        let pid = std::process::id();

        {
            let _guard = InstanceGuard::acquire_at(path.clone(), pid).unwrap();
            assert!(path.exists());
        }

        assert!(!path.exists());
    }

    #[test]
    fn clears_stale_lock_with_invalid_pid() {
        let path = test_lock_path("stale-invalid");
        fs::write(&path, "not-a-pid\n").unwrap();

        let _guard = InstanceGuard::acquire_at(path.clone(), std::process::id()).unwrap();
        assert!(path.exists());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_existing_lock_for_same_pid() {
        let path = test_lock_path("same-pid");
        fs::write(&path, format!("{}\n", std::process::id())).unwrap();

        let result = InstanceGuard::acquire_at(path.clone(), std::process::id());
        assert!(matches!(
            result,
            Err(AcquireError::AlreadyRunning { pid: Some(pid) }) if pid == std::process::id()
        ));

        let _ = fs::remove_file(path);
    }
}
