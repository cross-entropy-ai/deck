use super::{
    real_kill, send_signal, AcquireError, InstanceGuard, KillError, GRACEFUL_KILL_TIMEOUT,
};
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

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
fn send_signal_zero_to_self_reports_alive() {
    // signal 0 never delivers, just checks deliverability.
    assert!(matches!(send_signal(std::process::id(), 0), Ok(())));
}

#[test]
fn real_kill_terminates_cooperative_child() {
    // `sleep` has no SIGTERM handler, so the default action (terminate)
    // fires immediately. real_kill should resolve well under the
    // graceful timeout without needing the SIGKILL fallback.
    let child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn sleep");
    let pid = child.id();

    // In production, `deck --force` is not the parent of the target
    // deck, so when the old deck exits it gets reaped by its own
    // parent shell and `kill(pid, 0)` returns ESRCH promptly. Here we
    // ARE the parent, so the child becomes a zombie (still a valid
    // signal target) until we wait() on it. Reap on a helper thread
    // so the poll in real_kill sees ESRCH as soon as the child dies.
    let reap = std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });

    let start = Instant::now();
    let result = real_kill(pid);
    let elapsed = start.elapsed();
    let _ = reap.join();

    assert!(matches!(result, Ok(())), "real_kill returned {result:?}");
    assert!(
        elapsed < Duration::from_secs(1),
        "cooperative child should terminate fast, took {elapsed:?}"
    );
}

#[test]
fn real_kill_falls_back_to_sigkill_for_stubborn_child() {
    // `trap '' TERM` tells the shell to ignore SIGTERM. The shell
    // stays parked in `sleep 30`, so real_kill has to time out and
    // escalate to SIGKILL. This is the safety-net path.
    let mut child = std::process::Command::new("sh")
        .args(["-c", "trap '' TERM; sleep 30"])
        .spawn()
        .expect("spawn sh");
    let pid = child.id();

    let start = Instant::now();
    let result = real_kill(pid);
    let elapsed = start.elapsed();

    assert!(matches!(result, Ok(())), "real_kill returned {result:?}");
    assert!(
        elapsed >= GRACEFUL_KILL_TIMEOUT,
        "stubborn child should force a fallback, but returned in {elapsed:?}"
    );

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
