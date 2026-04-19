mod app;
mod infra;
mod model;
mod ui;

pub(crate) use app::action;
pub(crate) use infra::{git, instance_guard, nesting_guard, pty, refresh, shutdown, tmux, update};
pub(crate) use model::{config, keybindings, state};
pub(crate) use ui::{bridge, layout, theme};

use std::io;
use std::process::Command;

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use instance_guard::{AcquireError, InstanceGuard};

struct ParsedArgs {
    force: bool,
}

fn main() -> io::Result<()> {
    let args = match parse_args() {
        Ok(Some(args)) => args,
        Ok(None) => return Ok(()),
        Err(code) => std::process::exit(code),
    };

    // Install the SIGTERM handler before we acquire the lock, so a
    // concurrent `deck --force` that targets us is handled as soon as
    // the flag lands rather than hitting the default terminate action.
    if let Err(err) = shutdown::install_sigterm_handler() {
        eprintln!("deck: failed to install SIGTERM handler: {err}");
    }

    let acquire_result = if args.force {
        InstanceGuard::acquire_forcing(std::process::id())
    } else {
        InstanceGuard::acquire(std::process::id())
    };

    let _instance_guard = match acquire_result {
        Ok(guard) => guard,
        Err(AcquireError::AlreadyRunning { pid: Some(pid) }) => {
            eprintln!("deck: another instance is already running (pid {pid})");
            eprintln!("Retry with `deck --force` or kill the previous instance.");
            std::process::exit(1);
        }
        Err(AcquireError::AlreadyRunning { pid: None }) => {
            eprintln!("deck: another instance is already running");
            eprintln!("Retry with `deck --force` or kill the previous instance.");
            std::process::exit(1);
        }
        Err(AcquireError::ForceKillDenied { pid }) => {
            eprintln!("deck: cannot terminate pid {pid}: permission denied");
            std::process::exit(1);
        }
        Err(AcquireError::Io(err)) => return Err(err),
    };

    // Preflight: is tmux available?
    let tmux_ok = Command::new("tmux").arg("-V").output().is_ok();
    if !tmux_ok {
        eprintln!("deck: tmux not found in PATH");
        std::process::exit(1);
    }

    // Ensure at least one session exists
    if tmux::list_sessions().is_empty() {
        let _ = Command::new("tmux")
            .args(["new-session", "-d", "-s", "default"])
            .status();
    }

    ratatui::run(|terminal| {
        execute!(
            io::stdout(),
            EnableMouseCapture,
            EnableBracketedPaste,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        let size = terminal.size()?;
        let mut app = app::App::new(size.width, size.height)?;
        let result = app.run(terminal);
        execute!(
            io::stdout(),
            DisableMouseCapture,
            DisableBracketedPaste,
            PopKeyboardEnhancementFlags
        )?;
        result
    })?;

    Ok(())
}

fn parse_args() -> Result<Option<ParsedArgs>, i32> {
    let mut force = false;

    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--version" | "-V" => {
                println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--help" | "-h" => {
                print_help();
                return Ok(None);
            }
            "--force" | "-f" => {
                force = true;
            }
            _ => {
                eprintln!("deck: unknown argument '{arg}'");
                eprintln!("Run `deck --help` for usage.");
                return Err(2);
            }
        }
    }

    Ok(Some(ParsedArgs { force }))
}

fn print_help() {
    println!(
        "{name} {version}\n\nUsage:\n  {name}              Launch the sidebar UI\n  {name} --force      Terminate an existing deck instance and take over\n  {name} --version    Print version\n  {name} --help       Show this help",
        name = env!("CARGO_PKG_NAME"),
        version = env!("CARGO_PKG_VERSION"),
    );
}
