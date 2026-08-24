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
    /// Manage the optional agent lifecycle hooks; the payload is the action
    /// and an optional single target ("local" or a configured host). No
    /// target = local plus every configured remote.
    Hooks(HooksAction, Option<String>),
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
    Hooks(HooksCmd),
    UpgradeSelf(UpgradeSelfCmd),
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum HooksAction {
    Install,
    Uninstall,
    Status,
}

/// install, remove, or inspect deck's agent status hooks (Claude Code, Codex)
#[derive(FromArgs)]
#[argh(subcommand, name = "hooks")]
struct HooksCmd {
    #[argh(subcommand)]
    sub: HooksSubcommand,
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum HooksSubcommand {
    Install(HooksInstallCmd),
    Uninstall(HooksUninstallCmd),
    Status(HooksStatusCmd),
}

/// install the hooks ("local", a configured host, or omit for everywhere)
#[derive(FromArgs)]
#[argh(subcommand, name = "install")]
struct HooksInstallCmd {
    /// the target; omit for local + every configured remote
    #[argh(positional)]
    host: Option<String>,
}

/// remove deck's hooks, leaving everything else in place
#[derive(FromArgs)]
#[argh(subcommand, name = "uninstall")]
struct HooksUninstallCmd {
    /// the target; omit for local + every configured remote
    #[argh(positional)]
    host: Option<String>,
}

/// show where the hooks are installed and whether they can run
#[derive(FromArgs)]
#[argh(subcommand, name = "status")]
struct HooksStatusCmd {
    /// the target; omit for local + every configured remote
    #[argh(positional)]
    host: Option<String>,
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
        Some(Subcommand::Hooks(c)) => match c.sub {
            HooksSubcommand::Install(i) => ParsedCommand::Hooks(HooksAction::Install, i.host),
            HooksSubcommand::Uninstall(u) => ParsedCommand::Hooks(HooksAction::Uninstall, u.host),
            HooksSubcommand::Status(s) => ParsedCommand::Hooks(HooksAction::Status, s.host),
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
    let (state, warning) = crate::lane_state::LaneState::load(&config);
    // Say it before the edit's own success line: the host list this is about
    // to rewrite is not the one the user left behind.
    if let Some(warning) = warning {
        eprintln!("deck: {warning}");
    }
    Ok(state)
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

/// Run `deck hooks <action>` against one target or all of them. Reports one
/// line per agent per target; the Codex trust notice prints once at the end
/// because it is the one visible consequence the user must expect later
/// (Codex blocks its next launch on trusting changed hooks).
pub(crate) fn run_hooks(action: HooksAction, host: Option<&str>) -> i32 {
    use crate::agent_hooks::{self, HookAgent, HookFs, LocalFs, Outcome, RemoteFs};

    let state = match load_lane_state() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("deck: cannot read config: {e}");
            return 1;
        }
    };
    let configured: Vec<String> = state.remotes.iter().map(|r| r.host.clone()).collect();
    let targets: Vec<String> = match host {
        Some("local") => vec!["local".to_string()],
        Some(h) => {
            if !configured.iter().any(|c| c == h) {
                eprintln!("deck: '{h}' is not a configured remote (see `deck remote list`)");
                return 1;
            }
            vec![h.to_string()]
        }
        None => std::iter::once("local".to_string())
            .chain(configured)
            .collect(),
    };

    let mut failed = false;
    let mut codex_changed: Vec<String> = Vec::new();
    for target in &targets {
        let local = LocalFs;
        let remote;
        let fs: &dyn HookFs = if target == "local" {
            &local
        } else {
            remote = RemoteFs {
                host: target.clone(),
            };
            &remote
        };
        println!("{target}:");
        match action {
            HooksAction::Install | HooksAction::Uninstall => {
                let reports = if action == HooksAction::Install {
                    agent_hooks::install(fs)
                } else {
                    agent_hooks::uninstall(fs)
                };
                for report in reports {
                    let agent = report.agent.label();
                    match &report.outcome {
                        Outcome::Absent => {
                            println!("  {agent}: skipped (no ~/.{agent} — not set up here)")
                        }
                        Outcome::Installed => println!("  {agent}: installed"),
                        Outcome::Updated => println!("  {agent}: updated"),
                        Outcome::Unchanged => println!("  {agent}: already installed, unchanged"),
                        Outcome::Removed => println!("  {agent}: removed"),
                        Outcome::NothingToRemove => println!("  {agent}: nothing installed"),
                        Outcome::Failed(e) => {
                            failed = true;
                            eprintln!("  {agent}: FAILED — {e}");
                        }
                    }
                    if report.agent == HookAgent::Codex
                        && matches!(report.outcome, Outcome::Installed | Outcome::Updated)
                    {
                        codex_changed.push(target.clone());
                    }
                }
            }
            HooksAction::Status => {
                // Live proof the hooks actually run: `@deck_hook_alive` is
                // written on SessionStart, so "entries current" plus zero
                // reporting panes while agents are visibly running points at
                // Codex's trust gate (hooks installed but never trusted).
                let alive = if target == "local" {
                    crate::tmux::hook_alive_panes()
                } else {
                    crate::remote_tmux::hook_alive_panes(target)
                };
                match alive {
                    Some(n) => println!("  live: {n} pane(s) currently reporting hook state"),
                    None => println!("  live: (no tmux server reachable to ask)"),
                }
                for st in agent_hooks::status(fs) {
                    let agent = st.agent.label();
                    if let Some(e) = st.error {
                        failed = true;
                        eprintln!("  {agent}: cannot probe — {e}");
                        continue;
                    }
                    match st.installed {
                        None => println!("  {agent}: not set up here (no ~/.{agent})"),
                        Some((version, entries_ok)) => {
                            let script = match version {
                                Some(v) if v == agent_hooks::DECK_HOOK_VERSION => {
                                    format!("script v{v}")
                                }
                                Some(v) => format!(
                                    "script v{v} (deck ships v{})",
                                    agent_hooks::DECK_HOOK_VERSION
                                ),
                                None => "no script".to_string(),
                            };
                            let entries = if entries_ok {
                                "entries current"
                            } else {
                                "entries missing or stale"
                            };
                            let disabled = if st.hooks_disabled {
                                " — hooks disabled in ~/.codex/config.toml, they will not run"
                            } else {
                                ""
                            };
                            println!("  {agent}: {script}, {entries}{disabled}");
                        }
                    }
                }
            }
        }
    }

    if action == HooksAction::Install && !codex_changed.is_empty() {
        println!();
        println!(
            "note: Codex reviews new or changed hooks — the next `codex` launch on {} will ask once to trust deck's hooks (they only report agent status to tmux; choose \"Review hooks\" to read them).",
            codex_changed.join(", ")
        );
    }
    if failed {
        1
    } else {
        0
    }
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
