use std::env;
use std::io;

use crate::config::Config;
use crate::infra::command::{CommandError, CommandRunner, RealRunner};
use crate::tmux;

/// Startup preflight guard for checks that must pass before deck enters the TUI.
pub struct PreflightGuard;

impl PreflightGuard {
    pub fn run(attach_override: Option<&str>) -> Result<(), String> {
        run_preflight_checks_with_runner(&RealRunner, attach_override)
    }
}

fn run_preflight_checks_with_runner(
    runner: &dyn CommandRunner,
    attach_override: Option<&str>,
) -> Result<(), String> {
    ensure_tmux_available(runner)?;
    ensure_config_valid()?;
    ensure_requested_session(attach_override)?;
    ensure_at_least_one_session(runner)
}

fn ensure_tmux_available(runner: &dyn CommandRunner) -> Result<(), String> {
    runner
        .run("tmux", &["-V"], tmux::TMUX_TIMEOUT)
        .map(|_| ())
        .map_err(format_tmux_availability_error)
}

fn format_tmux_availability_error(err: CommandError) -> String {
    match err {
        CommandError::Spawn { source, .. } if source.kind() == io::ErrorKind::NotFound => {
            "tmux not found in PATH".to_string()
        }
        other => format!("tmux availability check failed: {other}"),
    }
}

fn ensure_config_valid() -> Result<(), String> {
    Config::try_load()
        .map(|_| ())
        .map_err(|err| format!("invalid config: {err}"))
}

/// `deck new <name>` creates the requested session before the app starts.
/// Duplicate names are hard errors so we don't silently coalesce with an
/// existing session that happens to share the name.
fn ensure_requested_session(name: Option<&str>) -> Result<(), String> {
    let Some(name) = name else {
        return Ok(());
    };

    if tmux::list_sessions().iter().any(|s| s.name == name) {
        return Err(format!("session '{name}' already exists"));
    }

    let cwd =
        env::current_dir().map_err(|err| format!("cannot determine current directory: {err}"))?;
    tmux::new_session(name, &cwd.to_string_lossy())
        .map(|_| ())
        .ok_or_else(|| format!("failed to create session '{name}'"))
}

fn ensure_at_least_one_session(runner: &dyn CommandRunner) -> Result<(), String> {
    if !tmux::list_sessions().is_empty() {
        return Ok(());
    }

    runner
        .run(
            "tmux",
            &["new-session", "-d", "-s", "default"],
            tmux::TMUX_TIMEOUT,
        )
        .map(|_| ())
        .map_err(|err| format!("failed to create default session: {err}"))
}

#[cfg(test)]
#[path = "../../tests/unit/infra/preflight_guard.rs"]
mod tests;
