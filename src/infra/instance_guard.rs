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
mod tests {
    use super::{AcquireError, InstanceGuard, KillError};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn test_lock_path(name: &str) -> PathBuf {
        PathBuf::from(format!("/tmp/deck-test-{name}-{}.lock", std::process::id()))
    }

    fn never_kill(_pid: u32) -> Result<(), KillError> {
        panic!("kill should not be called for stale/corrupt locks");
    }

    static KILL_CALLS: AtomicU32 = AtomicU32::new(0);

    fn counting_kill(_pid: u32) -> Result<(), KillError> {
        KILL_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn permission_denied_kill(_pid: u32) -> Result<(), KillError> {
        Err(KillError::PermissionDenied)
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

    #[test]
    fn force_acquires_over_corrupt_lock_without_killing() {
        let path = test_lock_path("force-corrupt");
        fs::write(&path, "garbage\n").unwrap();

        let _guard =
            InstanceGuard::acquire_forcing_at(path.clone(), std::process::id(), never_kill)
                .unwrap();
        assert!(path.exists());
    }

    #[test]
    fn force_acquires_over_stale_non_deck_pid_without_killing() {
        // Use PID 1 (init) — it exists but its command is not "deck", so
        // `pid_looks_like_deck` returns false and we must NOT try to kill it.
        let path = test_lock_path("force-stale-nondeck");
        fs::write(&path, "1\n").unwrap();

        let _guard =
            InstanceGuard::acquire_forcing_at(path.clone(), std::process::id(), never_kill)
                .unwrap();
        assert!(path.exists());
    }

    #[test]
    fn force_rejects_when_lock_holds_own_pid() {
        let path = test_lock_path("force-self-pid");
        fs::write(&path, format!("{}\n", std::process::id())).unwrap();

        let result =
            InstanceGuard::acquire_forcing_at(path.clone(), std::process::id(), never_kill);
        assert!(matches!(
            result,
            Err(AcquireError::AlreadyRunning { pid: Some(pid) }) if pid == std::process::id()
        ));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn force_kills_and_acquires_when_lock_holds_deck_pid() {
        let path = test_lock_path("force-kill-deck");
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg("exec -a deck-test-victim sleep 30")
            .spawn()
            .unwrap();
        let victim_pid = child.id();
        std::thread::sleep(std::time::Duration::from_millis(100));

        fs::write(&path, format!("{victim_pid}\n")).unwrap();
        KILL_CALLS.store(0, Ordering::SeqCst);

        let _guard =
            InstanceGuard::acquire_forcing_at(path.clone(), std::process::id(), counting_kill)
                .unwrap();

        assert_eq!(KILL_CALLS.load(Ordering::SeqCst), 1);
        assert!(path.exists());

        unsafe {
            libc::kill(victim_pid as libc::pid_t, libc::SIGKILL);
        }
        let _ = child.wait();
    }

    #[test]
    fn force_surfaces_permission_denied() {
        let path = test_lock_path("force-eperm");
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg("exec -a deck-test-eperm sleep 30")
            .spawn()
            .unwrap();
        let victim_pid = child.id();
        std::thread::sleep(std::time::Duration::from_millis(100));

        fs::write(&path, format!("{victim_pid}\n")).unwrap();

        let result = InstanceGuard::acquire_forcing_at(
            path.clone(),
            std::process::id(),
            permission_denied_kill,
        );
        assert!(matches!(
            result,
            Err(AcquireError::ForceKillDenied { pid }) if pid == victim_pid
        ));

        unsafe {
            libc::kill(victim_pid as libc::pid_t, libc::SIGKILL);
        }
        let _ = child.wait();
        let _ = fs::remove_file(path);
    }
}
