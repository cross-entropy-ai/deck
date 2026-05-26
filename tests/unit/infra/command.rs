use super::*;
use std::time::{Duration, Instant};

#[test]
fn real_runner_returns_stdout_on_success() {
    let runner = RealRunner;
    let out = runner
        .run("echo", &["hello"], Duration::from_secs(2))
        .expect("echo should succeed");
    assert_eq!(out.stdout_trimmed(), "hello");
}

#[test]
fn real_runner_classifies_nonzero_exit() {
    let runner = RealRunner;
    // `false` always exits non-zero.
    let err = runner
        .run("false", &[], Duration::from_secs(2))
        .expect_err("false should fail");
    match err {
        CommandError::NonZero { status, .. } => {
            assert!(!status.success());
        }
        other => panic!("expected NonZero, got {other:?}"),
    }
}

#[test]
fn real_runner_classifies_spawn_failure() {
    let runner = RealRunner;
    let err = runner
        .run("deck-nonexistent-binary-xyzzy", &[], Duration::from_secs(1))
        .expect_err("nonexistent binary should fail to spawn");
    assert!(matches!(err, CommandError::Spawn { .. }));
}

#[test]
fn real_runner_kills_after_timeout() {
    let runner = RealRunner;
    let start = Instant::now();
    let err = runner
        .run("sleep", &["10"], Duration::from_millis(150))
        .expect_err("sleep should time out");
    let elapsed = start.elapsed();
    assert!(matches!(err, CommandError::Timeout { .. }));
    // The kill path must actually unblock us within a small grace
    // window — if this regresses to "wait for the full sleep" we want
    // the test to fail.
    assert!(
        elapsed < Duration::from_secs(2),
        "timeout path took too long: {elapsed:?}"
    );
}

