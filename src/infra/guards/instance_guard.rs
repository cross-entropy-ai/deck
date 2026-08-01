use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use crate::tmux;

/// How long we wait for the old deck to quit after SIGTERM before falling back
/// to SIGKILL. The handler flips a flag the event loop reads on its next ~16ms
/// tick, so the old instance normally exits well under this budget.
const GRACEFUL_KILL_TIMEOUT: Duration = Duration::from_secs(2);
const GRACEFUL_KILL_POLL: Duration = Duration::from_millis(50);

#[derive(Debug)]
pub enum AcquireError {
    Io(io::Error),
    StaleLockCleanup { path: PathBuf, source: io::Error },
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
    /// Acquire the lock on behalf of the running process. `force` takes over
    /// (terminating a previous deck instance) instead of failing if the lock
    /// is held. The `*_at` variants take an injectable path/kill for tests.
    pub fn acquire_for_current_process(force: bool) -> Result<Self, AcquireError> {
        let pid = std::process::id();
        let path = Self::default_lock_path();
        if force {
            Self::acquire_forcing_at(path, pid, real_kill)
        } else {
            Self::acquire_at(path, pid)
        }
    }

    /// Like [`acquire_for_current_process`], but on a contention error (another
    /// instance running, or force-kill denied) it prints the diagnostic and
    /// exits. Genuine I/O errors are returned for the caller to propagate.
    ///
    /// [`acquire_for_current_process`]: Self::acquire_for_current_process
    pub fn acquire_for_current_process_or_exit(force: bool) -> io::Result<Self> {
        match Self::acquire_for_current_process(force) {
            Ok(guard) => Ok(guard),
            Err(AcquireError::AlreadyRunning { pid }) => {
                match pid {
                    Some(pid) => eprintln!("deck: another instance is already running (pid {pid})"),
                    None => eprintln!("deck: another instance is already running"),
                }
                eprintln!("Retry with `deck --force` or kill the previous instance.");
                std::process::exit(1);
            }
            Err(AcquireError::ForceKillDenied { pid }) => {
                eprintln!("deck: cannot terminate pid {pid}: permission denied");
                std::process::exit(1);
            }
            Err(AcquireError::StaleLockCleanup { path, source }) => Err(io::Error::new(
                source.kind(),
                format!(
                    "failed to remove stale instance lock {}: {source}",
                    path.display()
                ),
            )),
            Err(AcquireError::Io(err)) => Err(err),
        }
    }

    fn acquire_at(lock_path: PathBuf, current_pid: u32) -> Result<Self, AcquireError> {
        Self::acquire_at_with(lock_path, current_pid, remove_lock_file)
    }

    fn acquire_at_with(
        lock_path: PathBuf,
        current_pid: u32,
        remove_fn: fn(&Path) -> io::Result<()>,
    ) -> Result<Self, AcquireError> {
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
                            Self::remove_stale_lock(&lock_path, remove_fn)?;
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
        Self::acquire_forcing_at_with(lock_path, current_pid, kill_fn, remove_lock_file)
    }

    fn acquire_forcing_at_with(
        lock_path: PathBuf,
        current_pid: u32,
        kill_fn: fn(u32) -> Result<(), KillError>,
        remove_fn: fn(&Path) -> io::Result<()>,
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

        Self::remove_stale_lock(&lock_path, remove_fn)?;
        Self::acquire_at_with(lock_path, current_pid, remove_fn)
    }

    fn default_lock_path() -> PathBuf {
        let xdg_runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        let fallback_dir = fallback_lock_dir();
        Self::lock_path_for(
            xdg_runtime_dir.as_deref(),
            &fallback_dir,
            current_user_id(),
            runtime_dir_is_suitable,
        )
    }

    fn lock_path_for<F>(
        xdg_runtime_dir: Option<&Path>,
        temp_dir: &Path,
        uid: u32,
        runtime_dir_is_suitable: F,
    ) -> PathBuf
    where
        F: Fn(&Path, u32) -> bool,
    {
        if let Some(runtime_dir) =
            xdg_runtime_dir.filter(|path| path.is_absolute() && runtime_dir_is_suitable(path, uid))
        {
            return runtime_dir.join(format!("{}.lock", env!("CARGO_PKG_NAME")));
        }

        temp_dir.join(format!("{}-{uid}.lock", env!("CARGO_PKG_NAME")))
    }

    fn remove_stale_lock(
        lock_path: &Path,
        remove_fn: fn(&Path) -> io::Result<()>,
    ) -> Result<(), AcquireError> {
        match remove_fn(lock_path) {
            Ok(()) => Ok(()),
            // Another contender may have removed the stale lock first. The
            // following create_new attempt safely decides which process wins.
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(AcquireError::StaleLockCleanup {
                path: lock_path.to_path_buf(),
                source,
            }),
        }
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

fn remove_lock_file(path: &Path) -> io::Result<()> {
    fs::remove_file(path)
}

#[cfg(unix)]
fn fallback_lock_dir() -> PathBuf {
    PathBuf::from("/tmp")
}

#[cfg(not(unix))]
fn fallback_lock_dir() -> PathBuf {
    std::env::temp_dir()
}

#[cfg(unix)]
fn current_user_id() -> u32 {
    unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn current_user_id() -> u32 {
    std::process::id()
}

#[cfg(unix)]
fn runtime_dir_is_suitable(path: &Path, uid: u32) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_dir() && metadata.uid() == uid && metadata.permissions().mode() & 0o077 == 0
}

#[cfg(not(unix))]
fn runtime_dir_is_suitable(path: &Path, _uid: u32) -> bool {
    path.is_dir()
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
    // Ask politely first: the target deck's SIGTERM handler (see
    // `infra::shutdown`) flips a flag its event loop turns into the normal
    // Action::Quit shutdown (terminal restored, lock removed via Drop). Fall
    // back to SIGKILL only if it doesn't exit within the timeout.
    match send_signal(pid, libc::SIGTERM) {
        Ok(()) => {}
        Err(KillError::NoSuchProcess) => return Ok(()),
        Err(other) => return Err(other),
    }

    let start = Instant::now();
    while start.elapsed() < GRACEFUL_KILL_TIMEOUT {
        std::thread::sleep(GRACEFUL_KILL_POLL);
        if matches!(send_signal(pid, 0), Err(KillError::NoSuchProcess)) {
            return Ok(());
        }
    }

    // Hung or swallowing SIGTERM — use the hammer. The lock file won't
    // be cleared via Drop in this path, but the caller's remove_file
    // sweep handles that.
    match send_signal(pid, libc::SIGKILL) {
        Ok(()) | Err(KillError::NoSuchProcess) => Ok(()),
        Err(other) => Err(other),
    }
}

fn send_signal(pid: u32, sig: libc::c_int) -> Result<(), KillError> {
    let ret = unsafe { libc::kill(pid as libc::pid_t, sig) };
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
#[path = "../../../tests/unit/infra/instance_guard.rs"]
mod tests;
