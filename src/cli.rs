//! CLI parsing: an `argh` type tree lowered into [`ParsedCommand`]. The argh
//! types stay private so the rest of deck never sees the parser's shape.
//! The `///` lines on argh structs are required — argh renders them as
//! `--help` text.

use std::io::{self, Write};

use argh::FromArgs;

use crate::config::{Config, RemoteConfig};
use crate::ssh;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ParsedArgs {
    pub(crate) force: bool,
    pub(crate) attach_override: Option<String>,
    /// `new` sets this (the session must not already exist); `attach` leaves
    /// it false (attach, creating only if absent).
    pub(crate) create_new: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ParsedCommand {
    Run(ParsedArgs),
    RemoteAdd(String),
    RemoteList,
    RemoteRemove(String),
    /// Hidden re-exec entrypoint: in-place self-upgrade to the given version
    /// via the `self_update` crate. Spawned by the running TUI inside the
    /// upgrade pane so its progress renders live.
    UpgradeSelf(String),
}

/// deck — a terminal sidebar for browsing and switching tmux sessions.
///
/// Run with no subcommand to launch the sidebar UI.
#[derive(FromArgs)]
struct Cli {
    /// terminate an existing deck instance and take over (the default)
    #[argh(switch, short = 'f')]
    force: bool,

    /// refuse to start if another deck instance is already running
    #[argh(switch)]
    no_force: bool,

    /// print version and exit
    #[argh(switch, short = 'V')]
    version: bool,

    #[argh(subcommand)]
    cmd: Option<Subcommand>,
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum Subcommand {
    Attach(AttachCmd),
    New(NewCmd),
    Remote(RemoteCmd),
    UpgradeSelf(UpgradeSelfCmd),
}

/// attach to a session, creating it in the current directory if absent
#[derive(FromArgs)]
#[argh(subcommand, name = "attach")]
struct AttachCmd {
    /// the session to attach to
    #[argh(positional)]
    session: String,
}

/// create a session in the current directory and attach (errors if it exists)
#[derive(FromArgs)]
#[argh(subcommand, name = "new")]
struct NewCmd {
    /// the session to create
    #[argh(positional)]
    session: String,
}

/// register, list, or remove remote SSH hosts whose tmux sessions deck surfaces
#[derive(FromArgs)]
#[argh(subcommand, name = "remote")]
struct RemoteCmd {
    #[argh(subcommand)]
    sub: RemoteSubcommand,
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum RemoteSubcommand {
    Add(RemoteAddCmd),
    List(RemoteListCmd),
    Remove(RemoteRemoveCmd),
}

/// register an SSH host (resolved via `ssh -G`) and offer to enable multiplexing
#[derive(FromArgs)]
#[argh(subcommand, name = "add")]
struct RemoteAddCmd {
    /// the SSH host to add
    #[argh(positional)]
    host: String,
}

/// list configured remote hosts
#[derive(FromArgs)]
#[argh(subcommand, name = "list")]
struct RemoteListCmd {}

/// remove a remote host from the config
#[derive(FromArgs)]
#[argh(subcommand, name = "remove")]
struct RemoteRemoveCmd {
    /// the SSH host to remove
    #[argh(positional)]
    host: String,
}

/// hidden re-exec entrypoint that performs an in-place self-upgrade
#[derive(FromArgs)]
#[argh(subcommand, name = "__upgrade-self")]
struct UpgradeSelfCmd {
    /// the version to upgrade to
    #[argh(positional)]
    version: String,
}

pub(crate) fn parse_args<I: IntoIterator<Item = String>>(
    args: I,
) -> Result<Option<ParsedCommand>, i32> {
    let argv: Vec<String> = args.into_iter().collect();
    let arg_strs: Vec<&str> = argv.iter().map(String::as_str).collect();

    let cli = match Cli::from_args(&["deck"], &arg_strs) {
        Ok(cli) => cli,
        // argh's EarlyExit: status Ok(()) = help/usage (stdout, exit 0),
        // Err(()) = parse error, which we map to deck's exit code 2.
        Err(early) => match early.status {
            Ok(()) => {
                print!("{}", early.output);
                return Ok(None);
            }
            Err(()) => {
                eprint!("{}", early.output);
                return Err(2);
            }
        },
    };

    if cli.version {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(None);
    }

    // Takeover is on by default; `--no-force` opts out; `--force` wins if both.
    let force = cli.force || !cli.no_force;

    let command = match cli.cmd {
        None => ParsedCommand::Run(ParsedArgs {
            force,
            attach_override: None,
            create_new: false,
        }),
        Some(Subcommand::Attach(c)) => ParsedCommand::Run(ParsedArgs {
            force,
            attach_override: Some(c.session),
            create_new: false,
        }),
        Some(Subcommand::New(c)) => ParsedCommand::Run(ParsedArgs {
            force,
            attach_override: Some(c.session),
            create_new: true,
        }),
        Some(Subcommand::Remote(c)) => match c.sub {
            RemoteSubcommand::Add(a) => ParsedCommand::RemoteAdd(a.host),
            RemoteSubcommand::List(_) => ParsedCommand::RemoteList,
            RemoteSubcommand::Remove(r) => ParsedCommand::RemoteRemove(r.host),
        },
        Some(Subcommand::UpgradeSelf(u)) => ParsedCommand::UpgradeSelf(u.version),
    };
    Ok(Some(command))
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

fn finish_remote_change(
    config: &Config,
    success_message: &str,
    save: impl FnOnce(&Config) -> Result<(), String>,
) -> i32 {
    match save(config) {
        Ok(()) => {
            println!("{success_message}");
            0
        }
        Err(e) => {
            eprintln!("deck: cannot save config: {e}");
            1
        }
    }
}

pub(crate) fn run_remote_add(host: &str) -> i32 {
    let mut config = match Config::try_load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("deck: cannot read config ({e}); refusing to modify it.");
            eprintln!("deck: fix ~/.config/deck/config.yaml by hand, then retry.");
            return 1;
        }
    };
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
        forwards: vec![],
    });
    finish_remote_change(
        &config,
        &format!("deck: added remote '{host}'."),
        Config::save,
    )
}

pub(crate) fn run_remote_list() {
    // try_load, not load: listing must never rewrite a malformed config.
    let config = match Config::try_load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("deck: cannot read config: {e}");
            return;
        }
    };
    if config.remotes.is_empty() {
        println!("(no remotes configured)");
        return;
    }
    for r in &config.remotes {
        println!("{}", r.host);
    }
}

pub(crate) fn run_remote_remove(host: &str) -> i32 {
    let mut config = match Config::try_load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("deck: cannot read config ({e}); refusing to modify it.");
            eprintln!("deck: fix ~/.config/deck/config.yaml by hand, then retry.");
            return 1;
        }
    };
    let before = config.remotes.len();
    config.remotes.retain(|r| r.host != host);
    if config.remotes.len() == before {
        eprintln!("deck: no remote named '{host}'");
        return 1;
    }
    finish_remote_change(
        &config,
        &format!("deck: removed remote '{host}'."),
        Config::save,
    )
}

#[cfg(test)]
mod tests {
    use super::{finish_remote_change, parse_args, ParsedArgs, ParsedCommand};
    use crate::config::Config;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn run(parts: &[&str]) -> ParsedArgs {
        match parse_args(args(parts)).unwrap().unwrap() {
            ParsedCommand::Run(a) => a,
            other => panic!("expected Run, got {other:?}"),
        }
    }

    // argh does the parsing; these cover the two decisions cli.rs adds on top.
    #[test]
    fn new_creates_but_attach_does_not() {
        assert!(run(&["new", "foo"]).create_new);
        assert!(!run(&["attach", "foo"]).create_new);
    }

    #[test]
    fn force_reconciliation() {
        assert!(run(&[]).force);
        assert!(!run(&["--no-force"]).force);
        assert!(run(&["--no-force", "--force"]).force);
    }

    #[test]
    fn remote_change_returns_failure_when_config_cannot_be_saved() {
        let code = finish_remote_change(&Config::default(), "must not succeed", |_| {
            Err("permission denied".to_string())
        });

        assert_eq!(code, 1);
    }
}
