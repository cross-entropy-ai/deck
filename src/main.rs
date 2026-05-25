mod app;
mod infra;
mod model;
mod ui;

pub(crate) use app::action;
pub(crate) use infra::{
    claude_state, git, hooks, instance_guard, nesting_guard, proc_status, pty, refresh,
    remote_tmux, self_update, shutdown, ssh, tmux, update,
};
pub(crate) use model::{config, keybindings, new_session, state};
pub(crate) use ui::{bridge, layout, theme};

use std::io::{self, Write};
use std::process::Command;

use config::{Config, RemoteConfig};

use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use instance_guard::{AcquireError, InstanceGuard};

#[derive(Debug, PartialEq, Eq)]
struct ParsedArgs {
    force: bool,
    attach_override: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum ParsedCommand {
    Run(ParsedArgs),
    HooksInstall,
    HooksUninstall,
    RemoteAdd(String),
    RemoteList,
    RemoteRemove(String),
}

fn main() -> io::Result<()> {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Some(ParsedCommand::Run(args))) => args,
        Ok(Some(ParsedCommand::HooksInstall)) => {
            if let Err(e) = hooks::run_install() {
                eprintln!("deck: hook install failed: {e}");
                std::process::exit(1);
            }
            return Ok(());
        }
        Ok(Some(ParsedCommand::HooksUninstall)) => {
            if let Err(e) = hooks::run_uninstall() {
                eprintln!("deck: hook uninstall failed: {e}");
                std::process::exit(1);
            }
            return Ok(());
        }
        Ok(Some(ParsedCommand::RemoteAdd(host))) => {
            std::process::exit(run_remote_add(&host));
        }
        Ok(Some(ParsedCommand::RemoteList)) => {
            run_remote_list();
            return Ok(());
        }
        Ok(Some(ParsedCommand::RemoteRemove(host))) => {
            std::process::exit(run_remote_remove(&host));
        }
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

    // `deck new <name>`: create the requested session up front, in the
    // caller's current directory. Duplicate-name is a hard error; this
    // is checked before the create so we don't silently coalesce with an
    // existing session that happens to share the name.
    if let Some(name) = &args.attach_override {
        if tmux::list_sessions().iter().any(|s| &s.name == name) {
            eprintln!("deck: session '{name}' already exists");
            std::process::exit(1);
        }
        let cwd = match std::env::current_dir() {
            Ok(path) => path,
            Err(err) => {
                eprintln!("deck: cannot determine current directory: {err}");
                std::process::exit(1);
            }
        };
        if tmux::new_session(name, &cwd.to_string_lossy()).is_none() {
            eprintln!("deck: failed to create session '{name}'");
            std::process::exit(1);
        }
    }

    // Ensure at least one session exists (no-op for the `new` path,
    // since we just created one above).
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
            EnableFocusChange,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        let size = terminal.size()?;
        let mut app = app::App::new(size.width, size.height, args.attach_override.clone())?;
        let result = app.run(terminal);
        execute!(
            io::stdout(),
            DisableMouseCapture,
            DisableBracketedPaste,
            DisableFocusChange,
            PopKeyboardEnhancementFlags
        )?;
        result
    })?;

    Ok(())
}

fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<Option<ParsedCommand>, i32> {
    let mut iter = args.into_iter().peekable();

    if let Some(first) = iter.peek() {
        if first == "hooks" {
            iter.next();
            return parse_hooks_args(iter);
        }
        if first == "remote" {
            iter.next();
            return parse_remote_args(iter);
        }
    }

    let mut force = false;
    let mut attach_override: Option<String> = None;

    while let Some(arg) = iter.next() {
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
            "new" => {
                if attach_override.is_some() {
                    eprintln!("deck: 'new' may only be specified once");
                    return Err(2);
                }
                let Some(name) = iter.next() else {
                    eprintln!("deck: 'new' requires a session name");
                    eprintln!("Run `deck --help` for usage.");
                    return Err(2);
                };
                if name.starts_with('-') {
                    eprintln!("deck: expected a session name after 'new', got '{name}'");
                    return Err(2);
                }
                if let Some(extra) = iter.peek() {
                    if !extra.starts_with('-') {
                        eprintln!("deck: unexpected argument '{extra}' after `new {name}`");
                        return Err(2);
                    }
                }
                attach_override = Some(name);
            }
            _ => {
                eprintln!("deck: unknown argument '{arg}'");
                eprintln!("Run `deck --help` for usage.");
                return Err(2);
            }
        }
    }

    Ok(Some(ParsedCommand::Run(ParsedArgs {
        force,
        attach_override,
    })))
}

fn parse_hooks_args<I: Iterator<Item = String>>(mut iter: I) -> Result<Option<ParsedCommand>, i32> {
    let Some(sub) = iter.next() else {
        eprintln!("deck: `hooks` requires a subcommand (install|uninstall).");
        eprintln!("Run `deck --help` for usage.");
        return Err(2);
    };
    if let Some(extra) = iter.next() {
        eprintln!("deck: unexpected argument '{extra}' after `hooks {sub}`.");
        return Err(2);
    }
    match sub.as_str() {
        "install" => Ok(Some(ParsedCommand::HooksInstall)),
        "uninstall" => Ok(Some(ParsedCommand::HooksUninstall)),
        "--help" | "-h" => {
            print_help();
            Ok(None)
        }
        other => {
            eprintln!("deck: unknown `hooks` subcommand '{other}'. Expected install|uninstall.");
            Err(2)
        }
    }
}

fn parse_remote_args<I: Iterator<Item = String>>(
    mut iter: I,
) -> Result<Option<ParsedCommand>, i32> {
    let Some(sub) = iter.next() else {
        eprintln!("deck: `remote` requires a subcommand (add|list|remove).");
        eprintln!("Run `deck --help` for usage.");
        return Err(2);
    };
    match sub.as_str() {
        "list" => {
            if let Some(extra) = iter.next() {
                eprintln!("deck: unexpected argument '{extra}' after `remote list`.");
                return Err(2);
            }
            Ok(Some(ParsedCommand::RemoteList))
        }
        "add" | "remove" => {
            let Some(host) = iter.next() else {
                eprintln!("deck: `remote {sub}` requires a host argument.");
                return Err(2);
            };
            if host.starts_with('-') {
                eprintln!("deck: expected a host name after `remote {sub}`, got '{host}'");
                return Err(2);
            }
            if let Some(extra) = iter.next() {
                eprintln!("deck: unexpected argument '{extra}' after `remote {sub} {host}`.");
                return Err(2);
            }
            Ok(Some(if sub == "add" {
                ParsedCommand::RemoteAdd(host)
            } else {
                ParsedCommand::RemoteRemove(host)
            }))
        }
        "--help" | "-h" => {
            print_help();
            Ok(None)
        }
        other => {
            eprintln!("deck: unknown `remote` subcommand '{other}'. Expected add|list|remove.");
            Err(2)
        }
    }
}

/// Prompt the user with `question` and return true on yes. Defaults to no
/// when stdin isn't a TTY or on read errors.
fn prompt_yes_no(question: &str) -> bool {
    print!("{question} (y/N) ");
    if io::stdout().flush().is_err() {
        return false;
    }
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn run_remote_add(host: &str) -> i32 {
    let mut config = Config::load();
    if config.remotes.iter().any(|r| r.host == host) {
        eprintln!("deck: remote '{host}' is already configured");
        return 1;
    }

    let cfg = match ssh::effective_config(host) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("deck: cannot read ssh config for '{host}': {e}");
            return 1;
        }
    };

    // `ssh -G` returns success even for unknown hosts (it falls through
    // to defaults). Surface the resolved hostname so the user knows what
    // deck will actually connect to.
    if let Some(hostname) = cfg.get("hostname") {
        println!("deck: resolved '{host}' -> {hostname}");
    }

    let status = ssh::MultiplexStatus::from_config(&cfg);
    if !status.is_enabled() {
        println!();
        println!("Connection multiplexing is NOT enabled for '{host}'.");
        println!("deck will run several ssh commands (list sessions, attach, refresh);");
        println!("without multiplexing every call re-authenticates, which is slow and");
        println!("may trigger repeated password / 2FA prompts.");
        println!();
        println!("Suggested ~/.ssh/config snippet:");
        let snippet = ssh::suggested_snippet(host);
        for line in snippet.lines() {
            println!("  {line}");
        }
        if prompt_yes_no("Append this to ~/.ssh/config?") {
            match ssh::append_to_ssh_config(&snippet) {
                Ok(path) => println!("deck: appended snippet to {}", path.display()),
                Err(e) => {
                    eprintln!("deck: failed to write ~/.ssh/config: {e}");
                    return 1;
                }
            }
        } else {
            println!("deck: skipped; you can add the snippet later by hand.");
        }
    } else {
        println!("deck: ssh multiplexing already enabled for '{host}'.");
    }

    config.remotes.push(RemoteConfig {
        host: host.to_string(),
    });
    config.save();
    println!("deck: added remote '{host}'.");
    0
}

fn run_remote_list() {
    let config = Config::load();
    if config.remotes.is_empty() {
        println!("(no remotes configured)");
        return;
    }
    for r in &config.remotes {
        println!("{}", r.host);
    }
}

fn run_remote_remove(host: &str) -> i32 {
    let mut config = Config::load();
    let before = config.remotes.len();
    config.remotes.retain(|r| r.host != host);
    if config.remotes.len() == before {
        eprintln!("deck: no remote named '{host}'");
        return 1;
    }
    config.save();
    println!("deck: removed remote '{host}'.");
    0
}

fn print_help() {
    let name = env!("CARGO_PKG_NAME");
    let version = env!("CARGO_PKG_VERSION");
    println!(
        "{name} {version}

Usage:
  {name}                       Launch the sidebar UI
  {name} new <session>         Create a session named <session> in the current
                               directory and attach to it
  {name} --force               Terminate an existing deck instance and take over
  {name} --version             Print version
  {name} --help                Show this help

Claude Code hooks:
  {name} hooks install         Install Claude Code state hooks into
                               ~/.claude/settings.json (used to drive the
                               Working / Waiting indicators in the sidebar)
  {name} hooks uninstall       Remove deck's Claude Code hooks

Remote hosts:
  {name} remote add <host>     Register an SSH host whose tmux sessions deck
                               should surface alongside local ones. <host> must
                               resolve via `ssh -G` — i.e. either an entry in
                               ~/.ssh/config or a directly-resolvable hostname.
                               The command then checks whether SSH connection
                               multiplexing (ControlMaster + ControlPath +
                               ControlPersist) is enabled for that host, and if
                               not, offers to append a recommended block to
                               ~/.ssh/config. Without multiplexing every deck
                               action would re-authenticate, which is slow and
                               can trigger repeated password / 2FA prompts.

                               On startup deck spawns a background
                               `ssh -tt <host> tmux attach` PTY per configured
                               host and lists its sessions in the sidebar.
                               Authentication must be non-interactive
                               (`BatchMode=yes` is forced) — set up ssh keys
                               or an agent in advance. tmux must be on the
                               remote PATH for non-interactive shells; deck
                               prepends common Homebrew / linuxbrew paths so
                               the typical macOS / Linux installs Just Work.

  {name} remote list           List configured remote hosts.
  {name} remote remove <host>  Remove a remote host from the config.",
    );
}

#[cfg(test)]
mod tests {
    use super::{parse_args, ParsedArgs, ParsedCommand};

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn new_with_name_yields_attach_override() {
        let result = parse_args(args(&["new", "foo"])).unwrap().unwrap();
        assert_eq!(
            result,
            ParsedCommand::Run(ParsedArgs {
                force: false,
                attach_override: Some("foo".to_string()),
            })
        );
    }

    #[test]
    fn new_without_name_is_usage_error() {
        let result = parse_args(args(&["new"]));
        assert_eq!(result, Err(2));
    }

    #[test]
    fn new_with_extra_positional_is_usage_error() {
        let result = parse_args(args(&["new", "foo", "bar"]));
        assert_eq!(result, Err(2));
    }

    #[test]
    fn force_before_new_combines() {
        let result = parse_args(args(&["--force", "new", "foo"])).unwrap().unwrap();
        assert_eq!(
            result,
            ParsedCommand::Run(ParsedArgs {
                force: true,
                attach_override: Some("foo".to_string()),
            })
        );
    }

    #[test]
    fn force_after_new_combines() {
        let result = parse_args(args(&["new", "foo", "--force"])).unwrap().unwrap();
        assert_eq!(
            result,
            ParsedCommand::Run(ParsedArgs {
                force: true,
                attach_override: Some("foo".to_string()),
            })
        );
    }

    #[test]
    fn plain_deck_has_no_override() {
        let result = parse_args(args(&[])).unwrap().unwrap();
        assert_eq!(
            result,
            ParsedCommand::Run(ParsedArgs {
                force: false,
                attach_override: None,
            })
        );
    }
}
