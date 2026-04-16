mod action;
mod app;
mod bridge;
mod config;
mod git;
mod instance_guard;
mod nesting_guard;
mod pty;
mod state;
mod theme;
mod tmux;
mod ui;

use std::io;
use std::process::Command;

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use instance_guard::{AcquireError, InstanceGuard};

fn main() -> io::Result<()> {
    if let Some(code) = handle_meta_args() {
        std::process::exit(code);
    }

    let _instance_guard = match InstanceGuard::acquire(std::process::id()) {
        Ok(guard) => guard,
        Err(AcquireError::AlreadyRunning { pid: Some(pid) }) => {
            eprintln!("deck: another instance is already running (pid {pid})");
            std::process::exit(1);
        }
        Err(AcquireError::AlreadyRunning { pid: None }) => {
            eprintln!("deck: another instance is already running");
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

fn handle_meta_args() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    let arg = args.next()?;
    if args.next().is_some() {
        return None;
    }

    match arg.as_str() {
        "--version" | "-V" => {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            Some(0)
        }
        "--help" | "-h" => {
            println!(
                "{} {}\n\nUsage:\n  {}            Launch the sidebar UI\n  {} --version  Print version\n  {} --help     Show this help",
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_NAME"),
            );
            Some(0)
        }
        _ => None,
    }
}
