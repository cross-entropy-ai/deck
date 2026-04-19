use super::{AcquireError, InstanceGuard, KillError};
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// Spawn a long-running child whose argv[0] matches our "looks like deck"
/// heuristic (anything containing "deck"). Using `CommandExt::arg0`
/// sets argv[0] atomically at exec time, so there's no shell-version or
/// race-with-exec flakiness the way `sh -c 'exec -a ... sleep'` had.
fn spawn_deck_named(name: &str) -> std::process::Child {
    std::process::Command::new("sleep")
        .arg0(name)
        .arg("30")
        .spawn()
        .expect("spawn sleep")
}

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
    let mut child = spawn_deck_named("deck-test-victim");
    let victim_pid = child.id();

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
    let mut child = spawn_deck_named("deck-test-eperm");
    let victim_pid = child.id();

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
