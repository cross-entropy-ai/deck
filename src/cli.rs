//! CLI parsing: an `argh` type tree lowered into [`ParsedCommand`]. The argh
//! types stay private so the rest of deck never sees the parser's shape.
//! The `///` lines on argh structs are required — argh renders them as
//! `--help` text.

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

/// register an SSH host resolved via `ssh -G`
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

fn finish_remote_change(
    state: &crate::lane_state::LaneState,
    success_message: &str,
    save: impl FnOnce(&crate::lane_state::LaneState) -> Result<(), String>,
) -> i32 {
    match save(state) {
        Ok(()) => {
            println!("{success_message}");
            0
        }
        Err(e) => {
            eprintln!("deck: cannot save linked lanes: {e}");
            1
        }
    }
}

/// Load the remembered lanes for a CLI edit, seeding from the config the same
/// way the app does so `deck remote add` works on a first run after upgrading.
fn load_lane_state() -> Result<crate::lane_state::LaneState, String> {
    let config = Config::try_load()?;
    Ok(crate::lane_state::LaneState::load(&config))
}

pub(crate) fn run_remote_add(host: &str) -> i32 {
    let mut state = match load_lane_state() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("deck: cannot read config ({e}); refusing to modify it.");
            eprintln!("deck: fix ~/.config/deck/config.yaml by hand, then retry.");
            return 1;
        }
    };
    let mut remotes = state.to_remote_configs();
    if remotes.iter().any(|r| r.host == host) {
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

    remotes.push(RemoteConfig {
        host: host.to_string(),
        containers: vec![],
        forward_agent: true,
        forwards: vec![],
    });
    state.set_remote_configs(&remotes);
    finish_remote_change(
        &state,
        &format!("deck: added remote '{host}'."),
        crate::lane_state::LaneState::save,
    )
}

pub(crate) fn run_remote_list() {
    // try_load, not load: listing must never rewrite a malformed config.
    let state = match load_lane_state() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("deck: cannot read config: {e}");
            return;
        }
    };
    if state.remotes.is_empty() {
        println!("(no remotes linked)");
        return;
    }
    for remote in &state.remotes {
        println!("{}", remote.host);
    }
}

pub(crate) fn run_remote_remove(host: &str) -> i32 {
    let mut state = match load_lane_state() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("deck: cannot read config ({e}); refusing to modify it.");
            eprintln!("deck: fix ~/.config/deck/config.yaml by hand, then retry.");
            return 1;
        }
    };
    let before = state.remotes.len();
    state.remotes.retain(|remote| remote.host != host);
    if state.remotes.len() == before {
        eprintln!("deck: no remote named '{host}'");
        return 1;
    }
    finish_remote_change(
        &state,
        &format!("deck: removed remote '{host}'."),
        crate::lane_state::LaneState::save,
    )
}

#[cfg(test)]
mod tests {
    use super::{finish_remote_change, parse_args, ParsedArgs, ParsedCommand};

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
        let code = finish_remote_change(
            &crate::lane_state::LaneState::default(),
            "must not succeed",
            |_| Err("permission denied".to_string()),
        );

        assert_eq!(code, 1);
    }
}
