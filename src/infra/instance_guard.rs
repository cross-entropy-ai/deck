use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::tmux;

#[derive(Debug)]
pub enum AcquireError {
    Io(io::Error),
    AlreadyRunning { pid: Option<u32> },
    ForceKillDenied { pid: u32 },
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

    pub fn acquire_forcing(current_pid: u32) -> Result<Self, AcquireError> {
        Self::acquire_forcing_at(Self::default_lock_path(), current_pid, real_kill)
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
                        Some(pid) if pid != current_pid && tmux::pid_looks_like_deck(pid) => {
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

    fn acquire_forcing_at(
        lock_path: PathBuf,
        current_pid: u32,
        kill_fn: fn(u32) -> Result<(), KillError>,
    ) -> Result<Self, AcquireError> {
        match Self::create_lock(&lock_path, current_pid) {
            Ok(()) => {
                return Ok(Self {
                    lock_path,
                    pid: current_pid,
                });
            }
            Err(err) if err.kind() != io::ErrorKind::AlreadyExists => {
                return Err(AcquireError::Io(err));
            }
            _ => {}
        }

        let existing_pid = Self::read_lock_pid(&lock_path);
        if let Some(pid) = existing_pid {
            if pid == current_pid {
                return Err(AcquireError::AlreadyRunning { pid: Some(pid) });
            }
            if tmux::pid_looks_like_deck(pid) {
                eprintln!("deck: terminating previous instance (pid {pid})");
                match kill_fn(pid) {
                    Ok(()) | Err(KillError::NoSuchProcess) => {}
                    Err(KillError::PermissionDenied) => {
                        return Err(AcquireError::ForceKillDenied { pid });
                    }
                    Err(KillError::Other(err)) => {
                        return Err(AcquireError::Io(err));
                    }
                }
            }
        }

        let _ = fs::remove_file(&lock_path);
        Self::acquire_at(lock_path, current_pid)
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

#[derive(Debug)]
pub enum KillError {
    NoSuchProcess,
    PermissionDenied,
    Other(io::Error),
}

fn real_kill(pid: u32) -> Result<(), KillError> {
    let ret = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    if ret == 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::ESRCH) => Err(KillError::NoSuchProcess),
        Some(libc::EPERM) => Err(KillError::PermissionDenied),
        _ => Err(KillError::Other(err)),
    }
}

#[cfg(test)]
#[path = "../../tests/unit/infra/instance_guard.rs"]
mod tests;
