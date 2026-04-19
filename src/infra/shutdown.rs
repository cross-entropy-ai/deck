//! Cross-process shutdown nudge.
//!
//! `deck --force` sends SIGTERM to the running instance. The handler
//! installed here only flips an atomic flag — it does not do any
//! shutdown work itself, which would be unsafe from a signal handler.
//! The event loop in `App::run` polls the flag each tick and dispatches
//! `Action::Quit`, so the old instance exits through the exact same
//! path as the right-click "Quit" menu: terminal state restored,
//! `InstanceGuard` drops, lock file removed.
//!
//! Falling back to SIGKILL (in `instance_guard::real_kill`) stays as a
//! safety net for a hung or signal-ignoring process.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_sigterm(_: libc::c_int) {
    // Signal handlers must be async-signal-safe; atomic store is.
    SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
}

pub fn install_sigterm_handler() -> io::Result<()> {
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = handle_sigterm as *const () as libc::sighandler_t;
    // SA_RESTART keeps syscalls in the event loop (poll, read, write)
    // from bubbling EINTR when SIGTERM lands — the flag poll alone
    // is responsible for translating the signal into a shutdown.
    action.sa_flags = libc::SA_RESTART;
    unsafe {
        libc::sigemptyset(&mut action.sa_mask);
        if libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut()) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

pub fn shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::Relaxed)
}
