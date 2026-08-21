use super::{real_kill, AcquireError, InstanceGuard, KillError, GRACEFUL_KILL_TIMEOUT};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Spawn a long-running child that genuinely IS a binary named `deck`, so
/// `pid_looks_like_deck` recognizes it. That check matches the executable's
/// basename (`ps comm=`), not the argv, so a `sleep` with a `deck` argv[0]
/// does not count. Link `sleep` from a
/// temp path whose file name is exactly our package name and run that; the
/// returned dir holds the link and should be removed once the
/// child is reaped.
fn spawn_deck_named(tag: &str) -> (std::process::Child, PathBuf) {
    let dir = std::env::temp_dir().join(format!("deck-test-bin-{tag}-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let bin = dir.join(env!("CARGO_PKG_NAME"));
    std::os::unix::fs::symlink("/bin/sleep", &bin).expect("link sleep as a deck-named executable");
    let child = std::process::Command::new(&bin)
        .arg("30")
        .spawn()
        .expect("spawn deck-named binary");
    // `spawn` returns after fork, before the child is guaranteed to have
    // completed exec. The force-acquire path immediately reads `ps comm=`;
    // under parallel test load that could still report the test harness and
    // misclassify this live fixture as stale. Wait for the observable process
    // identity the production guard itself relies on.
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let comm = std::process::Command::new("ps")
            .args(["-p", &child.id().to_string(), "-o", "comm="])
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .unwrap_or_default();
        if Path::new(&comm).file_name().and_then(|name| name.to_str())
            == Some(env!("CARGO_PKG_NAME"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "deck-named child did not become observable: {comm:?}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    (child, dir)
}

fn test_lock_path(name: &str) -> PathBuf {
    PathBuf::from(format!("/tmp/deck-test-{name}-{}.lock", std::process::id()))
}

fn never_kill(_pid: u32) -> Result<(), KillError> {
    panic!("kill should not be called for stale/corrupt locks");
}

static KILL_CALLS: AtomicU32 = AtomicU32::new(0);
static PROCESS_TEST_LOCK: Mutex<()> = Mutex::new(());

fn counting_kill(_pid: u32) -> Result<(), KillError> {
    KILL_CALLS.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

fn permission_denied_kill(_pid: u32) -> Result<(), KillError> {
    Err(KillError::PermissionDenied)
}

fn permission_denied_remove(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "injected removal failure",
    ))
}

#[test]
fn lock_path_prefers_a_suitable_xdg_runtime_directory() {
    let runtime_dir = Path::new("/run/user/501");
    let path = InstanceGuard::lock_path_for(
        None,
        Some(runtime_dir),
        Path::new("/isolated-temp"),
        501,
        |path, uid| path == runtime_dir && uid == 501,
    );

    assert_eq!(path, runtime_dir.join("deck.lock"));
}

#[test]
fn fallback_lock_path_is_scoped_by_user_id() {
    let temp_dir = Path::new("/isolated-temp");
    let first = InstanceGuard::lock_path_for(None, None, temp_dir, 501, |_, _| false);
    let second = InstanceGuard::lock_path_for(None, None, temp_dir, 502, |_, _| false);

    assert_eq!(first, temp_dir.join("deck-501.lock"));
    assert_eq!(second, temp_dir.join("deck-502.lock"));
    assert_ne!(first, second);
}

#[test]
fn unsuitable_xdg_runtime_directory_uses_per_user_fallback() {
    let temp_dir = Path::new("/isolated-temp");
    let path = InstanceGuard::lock_path_for(
        None,
        Some(Path::new("/shared-runtime")),
        temp_dir,
        501,
        |_, _| false,
    );

    assert_eq!(path, temp_dir.join("deck-501.lock"));
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
fn stale_lock_removal_failure_is_returned_without_retrying() {
    let path = test_lock_path("stale-remove-denied");
    fs::write(&path, "not-a-pid\n").unwrap();

    let result =
        InstanceGuard::acquire_at_with(path.clone(), std::process::id(), permission_denied_remove);

    assert!(matches!(
        result,
        Err(AcquireError::StaleLockCleanup { path: error_path, source })
            if error_path == path && source.kind() == io::ErrorKind::PermissionDenied
    ));
    fs::remove_file(path).unwrap();
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
        InstanceGuard::acquire_forcing_at(path.clone(), std::process::id(), never_kill).unwrap();
    assert!(path.exists());
}

#[test]
fn force_surfaces_stale_lock_removal_failure() {
    let path = test_lock_path("force-remove-denied");
    fs::write(&path, "garbage\n").unwrap();

    let result = InstanceGuard::acquire_forcing_at_with(
        path.clone(),
        std::process::id(),
        never_kill,
        permission_denied_remove,
    );

    assert!(matches!(
        result,
        Err(AcquireError::StaleLockCleanup { path: error_path, source })
            if error_path == path && source.kind() == io::ErrorKind::PermissionDenied
    ));
    fs::remove_file(path).unwrap();
}

#[test]
fn force_acquires_over_stale_non_deck_pid_without_killing() {
    // Use PID 1 (init) — it exists but its command is not "deck", so
    // `pid_looks_like_deck` returns false and we must NOT try to kill it.
    let path = test_lock_path("force-stale-nondeck");
    fs::write(&path, "1\n").unwrap();

    let _guard =
        InstanceGuard::acquire_forcing_at(path.clone(), std::process::id(), never_kill).unwrap();
    assert!(path.exists());
}

#[test]
fn force_rejects_when_lock_holds_own_pid() {
    let path = test_lock_path("force-self-pid");
    fs::write(&path, format!("{}\n", std::process::id())).unwrap();

    let result = InstanceGuard::acquire_forcing_at(path.clone(), std::process::id(), never_kill);
    assert!(matches!(
        result,
        Err(AcquireError::AlreadyRunning { pid: Some(pid) }) if pid == std::process::id()
    ));

    let _ = fs::remove_file(path);
}

#[test]
fn force_kills_and_acquires_when_lock_holds_deck_pid() {
    let _serial = PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let path = test_lock_path("force-kill-deck");
    let (mut child, bindir) = spawn_deck_named("victim");
    let victim_pid = child.id();

    fs::write(&path, format!("{victim_pid}\n")).unwrap();
    KILL_CALLS.store(0, Ordering::SeqCst);

    let _guard =
        InstanceGuard::acquire_forcing_at(path.clone(), std::process::id(), counting_kill).unwrap();

    assert_eq!(KILL_CALLS.load(Ordering::SeqCst), 1);
    assert!(path.exists());

    unsafe {
        libc::kill(victim_pid as libc::pid_t, libc::SIGKILL);
    }
    let _ = child.wait();
    let _ = fs::remove_dir_all(&bindir);
}

#[test]
fn real_kill_terminates_cooperative_child() {
    let _serial = PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
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
    let _serial = PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
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
    let _serial = PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let path = test_lock_path("force-eperm");
    let (mut child, bindir) = spawn_deck_named("eperm");
    let victim_pid = child.id();

    fs::write(&path, format!("{victim_pid}\n")).unwrap();

    let result =
        InstanceGuard::acquire_forcing_at(path.clone(), std::process::id(), permission_denied_kill);
    assert!(matches!(
        result,
        Err(AcquireError::ForceKillDenied { pid }) if pid == victim_pid
    ));

    unsafe {
        libc::kill(victim_pid as libc::pid_t, libc::SIGKILL);
    }
    let _ = child.wait();
    let _ = fs::remove_dir_all(&bindir);
    let _ = fs::remove_file(path);
}

/// Two decks at once is exactly what the guard exists to prevent, so the way to
/// ask for it is explicit and absolute: a named lock file, which wins over both
/// the runtime directory and the temp fallback.
///
/// Without this there is no way to run a build under test beside the deck you
/// are working in — launching one kills the other (takeover is the default), and
/// `--no-force` only turns that into a refusal to start. Sandboxing `HOME` and
/// `XDG_STATE_HOME` does not reach the lock.
#[test]
fn a_named_lock_path_wins_over_the_shared_ones() {
    let named = Path::new("/tmp/deck-under-test.lock");

    assert_eq!(
        InstanceGuard::lock_path_for(
            Some(named),
            Some(Path::new("/shared-runtime")),
            Path::new("/isolated-temp"),
            501,
            |_, _| true,
        ),
        named,
    );

    // Relative would name a different lock depending on where deck was started,
    // and blank is an unset variable rather than the current directory — both
    // fall back to the shared answer.
    for ignored in ["relative/deck.lock", ""] {
        assert_eq!(
            InstanceGuard::lock_path_for(
                Some(Path::new(ignored)),
                None,
                Path::new("/isolated-temp"),
                501,
                |_, _| false,
            ),
            Path::new("/isolated-temp/deck-501.lock"),
            "{ignored:?} must not be taken for a lock path"
        );
    }
}

/// The point of the override is a lock nobody else is holding, so a directory
/// that does not exist yet is made rather than refused.
#[test]
fn a_named_lock_path_can_name_a_directory_that_is_not_there_yet() {
    let dir = std::env::temp_dir().join(format!("deck-lock-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let path = dir.join("nested/deck.lock");

    let guard = InstanceGuard::acquire_at(path.clone(), 4242).expect("acquire");
    assert!(path.exists(), "no lock at {path:?}");
    drop(guard);
    assert!(!path.exists(), "lock outlived its guard");

    let _ = fs::remove_dir_all(&dir);
}
