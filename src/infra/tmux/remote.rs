//! Tmux operations against a remote tmux server over SSH.
//!
//! Thin sibling of `infra::tmux`: same parsers and `SessionInfo` shape, but
//! each call shells out to `ssh <host> tmux ...`. Deck's connection options
//! are applied on every invocation so its Settings preference, rather than
//! `ssh_config`, controls whether calls reuse an SSH connection.
//!
//! Functions here take a *remote id*, not a bare host: either an ssh host,
//! or `host#container` addressing the tmux server *inside* a container on
//! that host (see [`parse_remote_id`]). For a container id, [`run_ssh`]
//! wraps the command in `<engine> exec … sh -c '…'`, so every snippet —
//! markers, switch/focus, captures — runs against the container's own
//! filesystem and tmux server; callers stay id-agnostic.

use std::collections::HashMap;
use std::time::Duration;

use crate::agent::DetectedAgent;
use crate::infra::command::{default_runner, CommandError, CommandRunner};
use crate::infra::parser::tmux::{
    exact_target, order_set_option_args, parse_sessions, SESSION_LIST_FORMAT_CONTAINER,
    SESSION_LIST_FORMAT_SSH,
};
use crate::model::session::SessionSnapshot;

/// Marker separating the pane-pid list from the `ps` snapshot in the
/// combined `agent_probe` ssh call. Must not start with `=` (zsh
/// equals-expansion treats `=word` as a command path) nor `-` (echo flag);
/// plain underscores are safe in any remote shell.
const AGENT_PROBE_MARKER: &str = "__DECK_AGENT_PROBE__";

/// Hard cap on a single remote ssh+tmux call. Generous vs the local 1s
/// budget because the first call may wait for the SSH master to come up.
pub const REMOTE_TIMEOUT: Duration = Duration::from_secs(5);

/// Separator between host and container in a *remote id* — the opaque string
/// the app's host-keyed plumbing (conn manager, markers, executor lanes)
/// carries for a container lane. `#` can appear in neither an ssh host alias
/// (ssh_config's comment character) nor a docker/podman container name
/// (`[a-zA-Z0-9][a-zA-Z0-9_.-]*`), so the parse is unambiguous.
pub const CONTAINER_SEP: char = '#';

/// The remote id for a container lane: `host#container`.
pub fn container_remote_id(host: &str, container: &str) -> String {
    format!("{host}{CONTAINER_SEP}{container}")
}

/// A parsed remote id: the ssh destination plus, for a container lane, the
/// container whose *inner* tmux server the id addresses. Everything above the
/// transport treats the id as opaque; only this module and the attach spawner
/// split it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteTarget<'a> {
    /// ssh destination (config alias or hostname).
    pub host: &'a str,
    /// Container on `host` to `exec` into, or `None` for the host itself.
    pub container: Option<&'a str>,
}

pub fn parse_remote_id(id: &str) -> RemoteTarget<'_> {
    match id.split_once(CONTAINER_SEP) {
        Some((host, container)) if !host.is_empty() && !container.is_empty() => RemoteTarget {
            host,
            container: Some(container),
        },
        _ => RemoteTarget {
            host: id,
            container: None,
        },
    }
}

/// Per-container transport settings, keyed by remote id. Written whenever the
/// app config is (re)applied (`TmuxSystem::configure`); read at the two
/// argv-assembly points (here and the attach spawner). Same shape as the
/// ForwardAgent registry in `crate::ssh` — callers pass opaque id strings, so
/// config can't be threaded through their signatures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerOpts {
    /// Container engine CLI on the host (`docker`/`podman`).
    pub engine: String,
    /// `SSH_AUTH_SOCK` value exported into the container on attach, when the
    /// user arranged for an agent socket to be reachable inside (bind mount).
    pub agent_sock: Option<String>,
}

impl Default for ContainerOpts {
    fn default() -> Self {
        Self {
            engine: crate::config::DEFAULT_CONTAINER_ENGINE.to_string(),
            agent_sock: None,
        }
    }
}

static CONTAINER_OPTS: std::sync::LazyLock<std::sync::RwLock<HashMap<String, ContainerOpts>>> =
    std::sync::LazyLock::new(|| std::sync::RwLock::new(HashMap::new()));

/// Replace the container-options table (startup and config reload).
pub fn set_container_opts(opts: HashMap<String, ContainerOpts>) {
    let mut table = CONTAINER_OPTS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *table = opts;
}

/// Add or replace one entry without disturbing the rest. Used when a container
/// is mounted at runtime, where rebuilding the whole table from config would
/// drop exactly the entry being added.
pub fn upsert_container_opts(remote_id: String, opts: ContainerOpts) {
    CONTAINER_OPTS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(remote_id, opts);
}

/// Transport settings for a container remote id; defaults (docker, no agent
/// socket) for ids the config no longer mentions — a stale in-flight call
/// then fails on docker's own error rather than panicking here.
pub(crate) fn container_opts(remote_id: &str) -> ContainerOpts {
    CONTAINER_OPTS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(remote_id)
        .cloned()
        .unwrap_or_default()
}

/// SSH options we apply on *every* remote call: Deck's connection-reuse block
/// plus `host`'s ForwardAgent setting. Both read live process-wide state
/// immediately before each spawn, so a change applies to later *connections*
/// without a restart. `host` is the ssh destination, never a container id — ssh
/// options are a property of the connection, not of what runs over it.
///
/// ForwardAgent must stay identical across every invocation for a host, and it
/// is not retroactive: whichever call opens the ControlMaster decides what the
/// multiplexed sessions get, because "the display and agent forwarded will be
/// the one belonging to the master connection" (ssh_config(5)). Deck's first
/// call to a host is normally a one-shot listing, not the attach — so dropping
/// ForwardAgent from the one-shots to narrow the exposure would create the
/// master without it and the feature would silently never work.
pub(crate) fn base_ssh_args(host: &str) -> Vec<String> {
    let mut args = crate::ssh::connection_opts();
    args.extend(
        crate::ssh::agent_forward_opts(host)
            .iter()
            .map(|opt| (*opt).to_string()),
    );
    args
}

/// PATH prelude prepended to every remote command, as its own `export`
/// statement. SSH's non-interactive shell skips `~/.zshrc` / `~/.profile`, so
/// non-default installs (Homebrew, linuxbrew, per-user) are invisible. Exporting
/// these paths ahead of the command makes deck work without editing remote
/// startup files. The trailing `$PATH` (expanded by the remote shell) keeps the
/// user's existing path intact.
///
/// An `export` statement, never the `PATH=… cmd` assignment-prefix spelling —
/// each of those two facts shipped as a bug:
///
/// - A prefix attaches to one *simple command*, so everything past the first
///   `;` runs with the login shell's PATH alone, and ahead of a compound (`if`,
///   `for`, `while`) the shell reads the reserved word as the command name and
///   dies with a syntax error.
/// - zsh — the default login shell on macOS — *restores* what a prefix
///   assignment set once the prefixed command returns, and does so even when
///   that command was `export`. `PATH=… export PATH=…`, which is what a leading
///   prefix plus a script's own export assembled to, therefore left PATH exactly
///   as zsh found it: `tmux` a few `;` later was `command not found` (exit 127)
///   on a Mac remote, while every bash host was fine (POSIX has assignments
///   before a special builtin persist, and bash obliges).
///
/// `$HOME/.local/bin` leads because it is the most specific: a user who put tmux
/// there meant that one. It matters most inside a container, where `sh -c` reads
/// no startup file and the image's `PATH` is only system directories — a tmux in
/// `~/.local/bin` was simply invisible and the lane failed with
/// `tmux: not found`. Fixing it here rather than by running a login shell keeps
/// the promise this constant exists to make: Deck does not depend on the remote's
/// startup files, which may not exist, may not be zsh, and may print banners into
/// output Deck parses.
///
/// `$HOME/.orbstack/bin` comes last, as a fallback rather than a preference: it
/// is where OrbStack — a container engine on macOS — keeps its `docker`, and it
/// reaches a normal shell only through `~/.zprofile`, which a login shell reads
/// and `ssh host cmd` does not. OrbStack usually also symlinks `docker` into
/// `/usr/local/bin` (already above), so this only carries a Mac remote whose
/// user declined that step — the difference between discovering its containers
/// and Deck reporting the host has no engine at all.
pub(crate) const REMOTE_PATH_EXPORT: &str = concat!(
    "export PATH=$HOME/.local/bin",
    ":/opt/homebrew/bin",
    ":/usr/local/bin",
    ":/home/linuxbrew/.linuxbrew/bin",
    ":$HOME/.orbstack/bin",
    ":$PATH"
);

/// How every remote tmux invocation spells `tmux`. `run_ssh` joins its argv
/// into one command line for the remote shell (and `container_exec_argv` joins
/// it again for the container's `sh`), so a two-word token is a single element
/// here and in the script-shaped calls alike.
///
/// `-u` states tmux's UTF-8 flag outright. Without it tmux infers UTF-8 from
/// the locale, and when it decides the client is *not* UTF-8 it runs its output
/// through `utf8_sanitize`, which replaces every byte it then considers
/// unprintable with `_`. A container command arrives through `<engine> exec`
/// with no locale at all — `LC_CTYPE=POSIX` — so the tab separating the fields
/// of every `-F` format came back as `_`, no session row parsed, and a
/// container lane with live sessions rendered as `(no sessions)`. Reported from
/// manual testing and confirmed against the real container: the same tmux 3.7b
/// answered `X\tY` over plain ssh (whose login shell has `LANG`) and `X_Y`
/// through `docker exec`.
///
/// Not fixed by exporting a locale instead: `LC_ALL=C.UTF-8` is silently
/// ignored when the image has no such locale installed — common in slim images
/// — and glibc falls back to C, which is exactly the state that broke. The flag
/// depends on nothing the image has to provide.
pub(crate) const REMOTE_TMUX: &str = "tmux -u";

/// Run `remote_argv` on the remote id's tmux server: on the host itself, or —
/// for a container id — inside the container via [`container_exec_argv`].
///
/// The [`REMOTE_PATH_EXPORT`] prelude goes first, as its own statement, so
/// callers hand over their command without one — a plain argv or a whole
/// `;`-separated script, either way every command in it resolves against the
/// same PATH.
pub(crate) fn run_ssh(
    runner: &dyn CommandRunner,
    remote_id: &str,
    remote_argv: &[&str],
) -> Result<String, CommandError> {
    let target = parse_remote_id(remote_id);
    let mut args = base_ssh_args(target.host);
    args.push(target.host.to_string());
    args.push(format!("{REMOTE_PATH_EXPORT} ;"));
    match target.container {
        None => args.extend(remote_argv.iter().map(|s| s.to_string())),
        Some(container) => args.extend(container_exec_argv(
            &container_opts(remote_id).engine,
            container,
            remote_argv,
            ContainerStdin::Detached,
        )),
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    runner
        .run("ssh", &arg_refs, REMOTE_TIMEOUT)
        .map(|out| out.stdout_trimmed())
}

/// Wrap a remote command for execution inside a container. The host shell
/// runs `<engine> exec <name> sh -c '<PATH prelude + command>'`: the inner
/// command is single-quoted into ONE host-shell word, so the container's
/// `sh` re-parses exactly the string the host shell would have — same PATH
/// prelude, same `;`/redirect semantics — and the marker/switch/capture
/// snippets work unchanged against the container's own filesystem. The engine
/// carries none of the host shell's environment through the `exec`, so the
/// prelude has to be stated again on the inside.
///
/// Container-bound commands must stay POSIX-sh clean: `sh` in slim images is
/// dash, which has no `$'…'` ANSI-C quoting — that's why `list_sessions` and
/// `agent_probe` pick `…_CONTAINER` spellings of their tmux formats.
///
/// `TERM` is passed for the same reason the attach path passes it — the engine
/// substitutes its own for the caller's, and tmux believes it (see
/// [`crate::pty::CHILD_TERM`]). It costs nothing here and keeps the two exec
/// spellings from drifting apart again.
fn container_exec_argv(
    engine: &str,
    container: &str,
    remote_argv: &[&str],
    stdin: ContainerStdin,
) -> Vec<String> {
    let inner = format!("{REMOTE_PATH_EXPORT} ; {}", remote_argv.join(" "));
    let mut args = vec![
        // Quoted like every other config value that reaches a remote shell (see
        // CLAUDE.md). Unquoted, `engine: "docker ; id > /tmp/x ; true"` ran on
        // the host every refresh tick — and it also disagreed with the attach
        // path, which always quoted, so a multi-word value listed sessions fine
        // and then could never open a PTY. `validate_container_engine` keeps the
        // value to one command, so quoting costs nothing legitimate.
        shell_single_quote(engine),
        "exec".to_string(),
    ];
    if stdin == ContainerStdin::Attached {
        args.push("-i".to_string());
    }
    args.extend([
        "-e".to_string(),
        shell_single_quote(&format!("TERM={}", crate::pty::CHILD_TERM)),
        shell_single_quote(container),
        "sh".to_string(),
        "-c".to_string(),
        shell_single_quote(&inner),
    ]);
    args
}

/// Whether the container `exec` keeps the caller's stdin attached.
///
/// Both engines detach stdin unless asked. Without `-i` the command inside the
/// container starts with its stdin already at EOF, so the staging `cat >` wrote
/// a 0-byte file while `cat`, `mv` and `printf` each still exited 0: Deck
/// reported the upload as done and pasted a path to nothing, and the agent in
/// the pane quietly dropped it back to text. Only a call that streams bytes asks
/// for `Attached`; every argv-only call runs with stdin at `/dev/null`
/// ([`crate::infra::command::RealRunner::run`]) and must not ask the engine to
/// hold a stream nothing writes to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ContainerStdin {
    Detached,
    Attached,
}

/// List tmux sessions on `host`.
///
/// - `Err(Unreachable)` — ssh couldn't reach the host (refused/timeout/auth/DNS,
///   reported as ssh's own exit 255 or a command timeout).
/// - `Err(Backend)` — the local ssh process or remote tmux command failed for
///   a non-connectivity reason.
/// - `Ok(empty)` — reachable but no tmux server (`list-sessions` exited
///   non-zero with "no server running").
/// - `Ok(non-empty)` — the live session list.
pub fn list_sessions(host: &str) -> Result<Vec<SessionSnapshot>, ListSessionsError> {
    list_sessions_with(default_runner(), host)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListSessionsError {
    Unreachable(String),
    Backend(String),
}

fn list_sessions_with(
    runner: &dyn CommandRunner,
    host: &str,
) -> Result<Vec<SessionSnapshot>, ListSessionsError> {
    // `$'...'` (bash/zsh ANSI-C quoting) makes the remote shell treat `#`
    // literally (no comment) and `\t` as a splittable tab byte; a container
    // command is re-parsed by POSIX `sh` inside the container instead, so it
    // takes the single-quoted literal-tab spelling. Trailing
    // `#{@deck_order}` carries deck's persisted display rank (empty when
    // unset). See `persist_session_order`.
    let is_container = parse_remote_id(host).container.is_some();
    let format = if is_container {
        SESSION_LIST_FORMAT_CONTAINER
    } else {
        SESSION_LIST_FORMAT_SSH
    };
    match run_ssh(runner, host, &[REMOTE_TMUX, "list-sessions", "-F", format]) {
        // No window-activity probe (unlike local): nothing reads remote
        // activity, so the extra `list-windows -a` roundtrip per host per
        // tick would be waste. Rows parse with `activity = 0`.
        Ok(raw) => Ok(parse_sessions(&raw, &HashMap::new())),
        // "no server running" is the *only* failure read as empty: the host
        // is reachable, just sessionless. A stopped/removed container reads
        // as unreachable (a placeholder row, not a warning every tick), like
        // a host that dropped off the network. Other non-zero exits (tmux
        // missing, permission, PATH) stay backend errors.
        Err(err) if is_no_server_error(&err) => Ok(Vec::new()),
        Err(err)
            if is_unreachable_error(&err)
                || (is_container && is_container_unavailable_error(&err)) =>
        {
            Err(ListSessionsError::Unreachable(err.to_string()))
        }
        Err(err) => Err(ListSessionsError::Backend(err.to_string())),
    }
}

/// Whether a failed *container* call means the container itself is gone,
/// stopped, or paused, rather than something inside it failing. Treated like an
/// unreachable host: deck shows the lane's placeholder row and retries on later
/// ticks instead of warning every tick.
///
/// Only ever consulted for a container id (see `list_sessions_with`): on a plain
/// host these phrases could come from unrelated rc-file noise on stderr, and
/// downgrading a real backend failure to "unreachable" would hide it behind a
/// permanent "(connecting…)" row.
///
/// Engines word this differently and none of the phrasings are contractual, so
/// match the several known spellings rather than one:
/// - docker stopped: `Container <id> is not running`
/// - docker paused: `Container <id> is paused, unpause the container before exec`
/// - docker/podman missing: `No such container` / `no such container`
/// - podman stopped: `can only create exec sessions on running containers:
///   container state improper`
fn is_container_unavailable_error(err: &CommandError) -> bool {
    let CommandError::NonZero { stderr, .. } = err else {
        return false;
    };
    let msg = String::from_utf8_lossy(stderr).to_lowercase();
    [
        "is not running",
        "no such container",
        "is paused",
        "on running containers",
        "container state improper",
    ]
    .iter()
    .any(|phrase| msg.contains(phrase))
}

/// Probe `host` for interactive agents in its tmux panes, in one ssh hop:
/// list panes, then a `ps` snapshot, separated by a marker; the pure
/// `crate::agent::detect_agents` does the rest (same as local, fed over
/// ssh). `None` if unreachable (section stays "probing"); `Some(empty)`
/// for a reachable host with no agents.
pub fn agent_probe(host: &str) -> Option<Vec<DetectedAgent>> {
    agent_probe_with(default_runner(), host)
}

fn agent_probe_with(runner: &dyn CommandRunner, host: &str) -> Option<Vec<DetectedAgent>> {
    // Commands joined by a bare `;` (shell separator, run in sequence).
    // `$'…'` protects the `#`/tabs in the tmux format on a host (bash/zsh);
    // inside a container the parser is POSIX `sh`, so the format is
    // single-quoted with literal tabs instead. `2>/dev/null` swallows tmux's
    // "no server" noise so a server-less target still yields a clean ps.
    // Marker must be shell-safe (see `AGENT_PROBE_MARKER`).
    let format = if parse_remote_id(host).container.is_some() {
        format!("'{}'", crate::infra::parser::pane::PANE_FORMAT)
    } else {
        format!(
            "$'{}'",
            crate::infra::parser::pane::PANE_FORMAT.replace('\t', "\\t")
        )
    };
    let raw = run_ssh(
        runner,
        host,
        &[
            REMOTE_TMUX,
            "list-panes",
            "-a",
            "-F",
            &format,
            "2>/dev/null",
            ";",
            "echo",
            AGENT_PROBE_MARKER,
            ";",
            // The compound's exit status is this last command's, and a
            // container image may ship no procps `ps` at all (debian-slim) or a
            // busybox one that rejects `-axo` — which would fail the whole
            // probe, so `run_ssh` errors and the lane's Agents section stays
            // stuck on "probing…" forever even though the pane list above
            // succeeded. Try the portable `-o` spelling next, then `true` so the
            // panes still parse and agents merely go undetected. No bare `ps`
            // fallback: its columns aren't `pid ppid args`, so it would feed the
            // detector garbage rather than nothing.
            "ps",
            "-axo",
            "pid=,ppid=,args=",
            "2>/dev/null",
            "||",
            "ps",
            "-o",
            "pid=,ppid=,args=",
            "2>/dev/null",
            "||",
            "true",
        ],
    )
    .ok()?;
    let (panes_part, ps_part) = raw.split_once(AGENT_PROBE_MARKER)?;
    let panes = crate::infra::parser::pane::parse_panes(panes_part);
    let mut agents = crate::agent::detect_agents(&panes, ps_part);

    // Classify each agent's status from its pane buffer. One batched hop
    // captures every agent pane at once (the panes are already known from
    // the probe), then the shared classifier runs — same as the local path.
    if !agents.is_empty() {
        let pane_ids: Vec<String> = agents.iter().map(|a| a.pane_id.clone()).collect();
        let buffers = capture_panes(host, &pane_ids);
        for a in &mut agents {
            if let Some(buf) = buffers.get(&a.pane_id) {
                a.status = crate::agent::classify_status(a.kind, buf);
            }
        }
    }
    Some(agents)
}

/// Container engines Deck probes when discovering what a host could mount, in
/// order. A host may have either or both installed; `docker` comes first because
/// it is the common case and [`crate::config::DEFAULT_CONTAINER_ENGINE`].
const CONTAINER_ENGINES: [&str; 2] = ["docker", "podman"];

/// Separates one engine's `ps` output from the next in the combined probe. Must
/// be shell-safe in any remote shell — see [`AGENT_PROBE_MARKER`].
const ENGINE_PROBE_MARKER: &str = "__DECK_ENGINE_PROBE__";

/// `ps -a` format. `|` needs no escaping once the whole format is single-quoted,
/// and it cannot occur in a container name (`[a-zA-Z0-9][a-zA-Z0-9_.-]*`) nor in
/// a state word — unlike `\t`, whose handling differs between engines and shells.
/// `.State` is the machine-readable field (`running`/`exited`/`paused`/…);
/// `.Status` is a human string like "Up 22 hours".
const CONTAINER_LIST_FORMAT: &str = "{{.State}}|{{.Names}}";

/// One container a host could mount as its own lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredContainer {
    pub name: String,
    /// The engine that reported it, so mounting talks to the right CLI.
    pub engine: String,
    /// Whether it is already running. Deck can only `exec` into a running
    /// container, so a stopped one needs starting first.
    pub running: bool,
}

/// Containers on `host`, running or not, across every engine it has.
///
/// Best-effort by design: a host with neither engine, or one where the daemon is
/// down, yields an empty list rather than an error — the picker then says the
/// host has nothing to offer, which is the same thing from the user's side. Runs
/// on a worker thread (one bounded ssh hop for both engines).
pub fn list_containers(host: &str) -> Vec<DiscoveredContainer> {
    list_containers_with(default_runner(), host)
}

fn list_containers_with(runner: &dyn CommandRunner, host: &str) -> Vec<DiscoveredContainer> {
    // Discovery always addresses the host itself, never a container: a container
    // cannot mount further containers.
    let host = parse_remote_id(host).host;
    let format = shell_single_quote(CONTAINER_LIST_FORMAT);
    // One hop for both engines, blocks separated by the marker so a missing
    // engine yields an empty block instead of failing the call, ending in `true`
    // so the compound's exit status is the probe's, not the last engine's.
    //
    // `join`, not push-if-not-first: the separator goes *between* engine blocks
    // and nowhere else. Deriving "not first" from the buffer instead put one
    // ahead of `docker` once the buffer stopped starting out empty, leaving an
    // empty command (`… ;  ; echo …`) that every POSIX shell
    // rejects outright — so the probe exited 2 and the `Err` arm below read it
    // as "this host has no engine", on every host.
    let probes: Vec<String> = CONTAINER_ENGINES
        .iter()
        .map(|engine| {
            format!(
                "{} ps -a --format {format} 2>/dev/null",
                shell_single_quote(engine)
            )
        })
        .collect();
    let script = format!(
        "{} ; true",
        probes.join(&format!(" ; echo {ENGINE_PROBE_MARKER} ; "))
    );

    let Ok(raw) = run_ssh(runner, host, &[script.as_str()]) else {
        return Vec::new();
    };
    parse_discovered_containers(&raw)
}

/// Split the combined probe into per-engine blocks and parse each. Names that
/// could not round-trip through a lane id are dropped rather than offered.
fn parse_discovered_containers(raw: &str) -> Vec<DiscoveredContainer> {
    let mut out: Vec<DiscoveredContainer> = Vec::new();
    for (engine, block) in CONTAINER_ENGINES.iter().zip(raw.split(ENGINE_PROBE_MARKER)) {
        for line in block.lines() {
            let Some((state, names)) = line.trim().split_once('|') else {
                continue;
            };
            // podman renders `.Names` as a list (`[web]`) and a container can
            // carry several names; take the first and drop the brackets.
            let name = names
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .next()
                .unwrap_or("")
                .trim();
            if crate::config::validate_container_name(name).is_err() {
                continue;
            }
            if out.iter().any(|found| found.name == name) {
                continue;
            }
            out.push(DiscoveredContainer {
                name: name.to_string(),
                engine: (*engine).to_string(),
                running: state.trim().eq_ignore_ascii_case("running"),
            });
        }
    }
    out
}

/// Separates the published-port answer from the container-IP answer in the
/// combined forward-target probe. Same rules as [`ENGINE_PROBE_MARKER`].
const FORWARD_PROBE_MARKER: &str = "__DECK_FORWARD_PROBE__";

/// Go template for the container's addresses, one per network, space-separated.
/// Single-quoted into the remote shell: `{`/`}` are safe but `#`-free quoting
/// keeps it of a piece with the other formats here.
const CONTAINER_IP_FORMAT: &str = "{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}";

/// Go template for the container's network mode. Read to recognize the one
/// case with no address of its own to find — see [`HOST_NETWORK_MODE`].
const CONTAINER_NETWORK_MODE_FORMAT: &str = "{{.HostConfig.NetworkMode}}";

/// The network mode that puts a container on the host's own stack. Such a
/// container has no address and publishes no ports — both of the other answers
/// are empty *by design* — and its services are reachable on the host's
/// loopback at the very port they bind.
const HOST_NETWORK_MODE: &str = "host";

/// Where a `-L` forward into `container` should point, as an `addr:port` the
/// *host* can reach. Resolved on every apply and never persisted: both answers
/// below change when the container restarts, so a stored one is a forward that
/// silently points at nothing.
///
/// Three answers, in order:
/// 1. The port the container **publishes**, if it publishes this one. Reaching a
///    published port needs nothing of the host's network but a loopback hop, so
///    it works everywhere — including a Docker Desktop host, where containers
///    live in a VM the host cannot route to at all.
/// 2. The **host's own loopback**, when the container runs on the host's network
///    stack (`--network host`). It has no address and publishes nothing, so
///    both other answers are empty and it would read as unreachable — while in
///    fact it is the *most* reachable case: the port it binds is the host's.
/// 3. The container's **own address**, otherwise. The host has to be able to
///    route to the container network for this; a Linux bridge can, and so can
///    OrbStack on macOS, by design.
///
/// None of them is an error the user has to see rather than a forward that
/// appears to work: `ssh -O forward` reports success as soon as the *local*
/// listener binds, so an unreachable endpoint would surface only as a
/// connection that hangs later, with nothing pointing back here.
pub fn container_forward_target(
    host: &str,
    engine: &str,
    container: &str,
    port: u16,
) -> Result<String, String> {
    container_forward_target_with(default_runner(), host, engine, container, port)
}

fn container_forward_target_with(
    runner: &dyn CommandRunner,
    host: &str,
    engine: &str,
    container: &str,
    port: u16,
) -> Result<String, String> {
    // The probe addresses the host, never the container: both answers are the
    // *engine's*, and the engine runs on the host.
    let host = parse_remote_id(host).host;
    let engine_q = shell_single_quote(engine);
    let name_q = shell_single_quote(container);
    // One hop for all three questions, marker-separated so an engine that
    // answers none yields empty blocks instead of failing the call. Ends in
    // `true` so the compound's status is the probe's, not the last command's.
    let script = format!(
        "{engine_q} port {name_q} {port} 2>/dev/null ; \
         echo {FORWARD_PROBE_MARKER} ; \
         {engine_q} inspect -f {mode} {name_q} 2>/dev/null ; \
         echo {FORWARD_PROBE_MARKER} ; \
         {engine_q} inspect -f {ips} {name_q} 2>/dev/null ; true",
        mode = shell_single_quote(CONTAINER_NETWORK_MODE_FORMAT),
        ips = shell_single_quote(CONTAINER_IP_FORMAT),
    );
    let raw = run_ssh(runner, host, &[script.as_str()])
        .map_err(|error| format!("could not ask {host} where {container} answers: {error}"))?;
    parse_forward_target(&raw, port).ok_or_else(|| {
        format!(
            "{container} does not publish port {port}, is not on {host}'s own network, and has no \
             address {host} can see — publish the port, or use a host whose network reaches the \
             container"
        )
    })
}

/// Pick a target out of the three probe blocks — published, network mode, then
/// the container's addresses. See [`container_forward_target`] for why the
/// order is what it is.
fn parse_forward_target(raw: &str, port: u16) -> Option<String> {
    let mut blocks = raw.split(FORWARD_PROBE_MARKER);
    let (published, mode, addresses) = (blocks.next()?, blocks.next()?, blocks.next()?);

    if let Some(mapping) = published
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
    {
        if let Some(target) = published_target(mapping) {
            return Some(target);
        }
    }
    // On the host's own stack a container binds the *host's* ports, so the port
    // asked for is the port to dial — there is no translation, and no address
    // of its own to go looking for.
    if mode.trim() == HOST_NETWORK_MODE {
        return Some(format!("127.0.0.1:{port}"));
    }
    let address = addresses.split_whitespace().find(|ip| !ip.is_empty())?;
    Some(format!("{address}:{port}"))
}

/// Turn one `<engine> port` mapping line into an `addr:port` the host can dial.
///
/// A wildcard bind is rewritten to loopback: `0.0.0.0` is where the container
/// *accepts*, not an address to connect to. Any other address is kept as
/// published — a port bound to one interface is not reachable on loopback, and
/// substituting one there would produce a forward that never connects.
fn published_target(mapping: &str) -> Option<String> {
    let (addr, port) = mapping.rsplit_once(':')?;
    if port.is_empty() || port.parse::<u16>().is_err() {
        return None;
    }
    let addr = addr.trim();
    let addr = match addr.trim_start_matches('[').trim_end_matches(']') {
        "0.0.0.0" | "::" | "" => "127.0.0.1",
        _ => addr,
    };
    Some(format!("{addr}:{port}"))
}

/// Start a stopped container so Deck can `exec` into it. Returns the engine's
/// own message on failure, which is what the user needs to see (permission
/// denied, no such container, daemon down).
pub fn start_container(host: &str, engine: &str, name: &str) -> Result<(), String> {
    start_container_with(default_runner(), host, engine, name)
}

fn start_container_with(
    runner: &dyn CommandRunner,
    host: &str,
    engine: &str,
    name: &str,
) -> Result<(), String> {
    let host = parse_remote_id(host).host;
    run_ssh(
        runner,
        host,
        &[
            shell_single_quote(engine).as_str(),
            "start",
            shell_single_quote(name).as_str(),
        ],
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

/// Marker line emitted before each pane's buffer in a batched capture. The
/// leading `_` keeps it clear of the remote-shell `=`/`-` traps.
const CAPTURE_MARKER: &str = "__deck_cap__";

/// Capture several remote panes in a SINGLE ssh hop, returning `pane_id ->
/// buffer`. Shared by the agent status probe and the summary generator so
/// neither pays one ssh roundtrip per pane. Empty map on failure / no panes.
/// Pane ids are deck-known `%N` handles.
pub(crate) fn capture_panes(host: &str, pane_ids: &[String]) -> HashMap<String, String> {
    if pane_ids.is_empty() {
        return HashMap::new();
    }
    let runner = default_runner();
    // One remote command: loop the panes, printing a marker line + each buffer.
    let ids = pane_ids
        .iter()
        .map(|p| shell_single_quote(p))
        .collect::<Vec<_>>()
        .join(" ");
    let script = format!(
        "for p in {ids}; do echo {marker} \"$p\"; \
         {tmux} capture-pane -p -J -t \"$p\" 2>/dev/null; done",
        marker = CAPTURE_MARKER,
        tmux = REMOTE_TMUX,
    );
    // Through `run_ssh` so a container id gets the exec wrapping, and so the
    // PATH export lands ahead of the loop — a `for` cannot take a leading
    // assignment at all, which is half of why the prelude is a statement.
    let Ok(out) = run_ssh(runner, host, &[script.as_str()]) else {
        return HashMap::new();
    };
    parse_captures(&out)
}

/// Split batched-capture stdout into `pane_id -> buffer` on the
/// `__deck_cap__ <id>` marker lines preceding each pane's content.
fn parse_captures(raw: &str) -> HashMap<String, String> {
    let prefix = format!("{CAPTURE_MARKER} ");
    let mut map = HashMap::new();
    let mut cur: Option<(String, String)> = None;
    for line in raw.lines() {
        if let Some(id) = line.strip_prefix(&prefix) {
            if let Some((k, v)) = cur.take() {
                map.insert(k, v);
            }
            cur = Some((id.trim().to_string(), String::new()));
        } else if let Some((_, buf)) = cur.as_mut() {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(line);
        }
    }
    if let Some((k, v)) = cur.take() {
        map.insert(k, v);
    }
    map
}

/// Whether a failed remote `tmux` call means "reachable host, no tmux
/// server up" — as opposed to ssh not reaching the host, or tmux failing
/// for some other reason we shouldn't paper over as "no sessions".
///
/// ssh reports its *own* failures (refused/timeout/auth/DNS) as exit 255;
/// tmux exits non-zero with a "no server" message. Require both: a
/// non-255 remote exit AND a stderr that names the missing server.
fn is_no_server_error(err: &CommandError) -> bool {
    let CommandError::NonZero { status, stderr, .. } = err else {
        return false;
    };
    if status.code() == Some(255) {
        return false;
    }
    let msg = String::from_utf8_lossy(stderr).to_lowercase();
    msg.contains("no server running")
        || msg.contains("failed to connect to server")
        || msg.contains("error connecting to")
}

fn is_unreachable_error(err: &CommandError) -> bool {
    match err {
        CommandError::Timeout { .. } => true,
        CommandError::NonZero { status, .. } => status.code() == Some(255),
        CommandError::Spawn { .. } => false,
    }
}

/// Single-quote a value so the remote shell treats it as one literal
/// token. ssh re-joins the argv into a string the login shell re-parses
/// (argv boundaries lost), so user-supplied names/paths must be quoted —
/// for spaces and to neutralize shell metacharacters. Embedded single
/// quotes are escaped as `'\''`.
pub(crate) fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Quote a remote path, preserving a leading `~` / `~/` as remote `$HOME`.
/// A single-quoted `~` won't expand and tmux's `-c` won't expand it
/// either, so the home prefix is emitted as `"$HOME"` (the only unquoted
/// part) and the rest single-quoted.
fn shell_quote_remote_path(path: &str) -> String {
    if path == "~" {
        return "\"$HOME\"".to_string();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return format!("\"$HOME\"/{}", shell_single_quote(rest));
    }
    shell_single_quote(path)
}

/// Remote-shell snippet reading Deck's recorded client tty for this
/// connection (`host` + `marker_id`) into shell var `C`. Prefixed to the
/// switch/focus command; both run tmux **only** when `C` is non-empty,
/// passing it as an explicit `-c "$C"` target. A missing marker (reconnect
/// race, or unwritable `~/.cache`) yields no tmux command rather than an
/// untargeted op that could move another client. Writing `-c "$C"` as two
/// shell words avoids the zsh `${C:+…}` word-splitting trap; the guarding
/// `[ -n "$C" ]` is portable.
pub(crate) fn read_client_tty(host: &str, marker_id: u64) -> String {
    format!(
        "C=$(cat {marker} 2>/dev/null)",
        marker = client_marker_token(host, marker_id),
    )
}

/// Sanitized `host` component for the marker filename: keep it shell-safe
/// (alphanumerics, `-`, `_`), everything else → `_`.
fn marker_host_part(host: &str) -> String {
    host.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Logical remote path of the per-*connection* file where Deck's attach
/// wrapper records the tty of *its* tmux client (the `ssh -tt` pty =
/// tmux's `#{client_tty}`). `switch-client` calls read it back as an
/// explicit `-c` target so they re-point Deck's own client, not whatever
/// tmux treats as current when several clients are attached.
///
/// Keyed by Deck's local pid + host + per-spawn `marker_id`. The id makes
/// it connection-scoped: each (re)connect allocates a fresh id, so a
/// switch/focus during the reconnect race reads the *new* path (absent
/// until the wrapper writes it → empty → safe fallback) and never picks up
/// the previous connection's stale tty. Lives under `~/.cache` (disposable).
fn client_marker_path(host: &str, marker_id: u64) -> String {
    let pid = std::process::id();
    format!(
        "~/.cache/deck/client-{pid}-{}-{marker_id}",
        marker_host_part(host)
    )
}

/// [`client_marker_path`] quoted for safe interpolation into a remote
/// shell command (`~` → `"$HOME"`). Used by both the attach wrapper
/// (writer, in `remote_spawn`) and the switch/focus calls (reader).
pub(crate) fn client_marker_token(host: &str, marker_id: u64) -> String {
    shell_quote_remote_path(&client_marker_path(host, marker_id))
}

/// `find -name` pattern matching all of this Deck process's marker files for
/// `host` (any `marker_id`). The attach wrapper passes this as a quoted `find`
/// argument instead of exposing a bare shell glob: zsh treats an unmatched
/// glob as a fatal expansion error before `rm -f` or its redirection can run.
/// The returned pattern is safe to single-quote (digits + sanitized host +
/// `*`; no quotes or shell metacharacters other than the wildcard interpreted
/// by `find`, not the login shell).
pub(crate) fn client_marker_name_pattern(host: &str) -> String {
    let pid = std::process::id();
    format!("client-{pid}-{}-*", marker_host_part(host))
}

/// The `~/.cache/deck` directory token the attach wrapper `mkdir -p`s
/// before writing the marker file.
pub(crate) fn client_cache_dir_token() -> String {
    shell_quote_remote_path("~/.cache/deck")
}

/// Quoted remote path of the stable symlink this deck process points at its
/// forwarded ssh-agent socket. Written by the attach prelude, read by every pane
/// through `SSH_AUTH_SOCK`.
///
/// Scoped to this process rather than a single fixed name, because the remote
/// home may be *shared*: with one name, two people decking into `deploy@host`
/// would take turns re-pointing the symlink, so a pane of Alice's would reach
/// Bob's forwarded agent and sign with his keys. A process-unique name also
/// means one deck exiting can only leave a *dangling* link behind, never one
/// aimed at a stranger's live agent.
///
/// Process-scoped is the right lifetime: reconnects reuse it (that is the whole
/// point — a pane keeps working across them), while a deck restart mints a new
/// one, and panes from the previous run had a dead agent the moment that
/// process's ssh connection went away regardless.
pub(crate) fn agent_socket_token() -> String {
    static NAME: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        // Two processes could share a pid across two machines reaching the same
        // account, so mix in a clock reading. Sanitized by construction: digits
        // and hex only.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.subsec_nanos());
        format!(
            "~/.ssh/deck-agent-{pid}-{nanos:08x}.sock",
            pid = std::process::id(),
        )
    });
    shell_quote_remote_path(&NAME)
}

/// Confirm out of band (not via the PTY stream) that this connection's
/// client-tty marker got written, so switch/focus commit only once their
/// `-c` target exists. Returns `true` iff the marker file is present and
/// non-empty.
///
/// Not inferred from PTY output: that stream can carry banners /
/// forced-command noise / arbitrary chunking before or instead of a
/// marker, so scanning for a sentinel both misses real markers and accepts
/// absent ones. A `[ -s marker ]` check answers exactly the question. The
/// connect race (marker written just after `ssh` connects) is covered by a
/// couple of in-shell retries — the wait runs remotely in one bounded ssh
/// call, capped by the ssh timeout, not app-side polling.
pub fn wait_for_client_marker(host: &str, marker_id: u64) -> bool {
    wait_for_client_marker_with(default_runner(), host, marker_id)
}

fn wait_for_client_marker_with(runner: &dyn CommandRunner, host: &str, marker_id: u64) -> bool {
    let marker = client_marker_token(host, marker_id);
    // Check now, then retry twice at 1s steps (integer `sleep` for POSIX).
    // The marker is written ~instantly so the first check usually wins; the
    // retries cover the race. Starts with `[` (a simple command) so
    // run_ssh's `PATH=…` prefix attaches cleanly; total wait < REMOTE_TIMEOUT.
    let cmd = format!(
        "[ -s {marker} ] || {{ sleep 1; [ -s {marker} ]; }} || {{ sleep 1; [ -s {marker} ]; }}"
    );
    run_ssh(runner, host, &[cmd.as_str()]).is_ok()
}

/// Tell the remote tmux server to switch *Deck's own* attached client to
/// `session`. Transport failures are returned to the session executor so the
/// UI can report them.
///
/// The client is targeted explicitly (`-c`) via the tty our attach
/// wrapper recorded for this host — see [`client_marker_token`] — so we
/// don't re-point whatever client tmux happens to consider "current"
/// when more than one client is attached to the same server.
pub fn switch_client(host: &str, marker_id: u64, session: &str) -> Result<(), CommandError> {
    switch_client_with(default_runner(), host, marker_id, session)
}

fn switch_client_with(
    runner: &dyn CommandRunner,
    host: &str,
    marker_id: u64,
    session: &str,
) -> Result<(), CommandError> {
    let target = shell_single_quote(&exact_target(session));
    // Switch only when the recorded tty is known, so we target Deck's OWN
    // client. An untargeted `switch-client` could re-point another client,
    // so a missing marker no-ops and a later call (after it lands) switches.
    let cmd = format!(
        "{read_c} ; [ -n \"$C\" ] && {REMOTE_TMUX} switch-client -c \"$C\" -t {target}",
        read_c = read_client_tty(host, marker_id),
    );
    run_ssh(runner, host, &[cmd.as_str()]).map(|_| ())
}

/// Test seam: run the unified focus rule over the remote (ssh) transport.
/// Production focus goes through [`crate::focus::run_focus`]; this wrapper
/// lets the remote-transport tests drive the shared rule with a
/// `FakeRunner` and assert the emitted ssh command's shape.
#[cfg(test)]
fn focus_pane_with(
    runner: &dyn CommandRunner,
    host: &str,
    marker_id: u64,
    session: &str,
    pane_id: &str,
) -> crate::tmux::PaneFocus {
    crate::focus::run_focus_with(
        runner,
        &crate::focus::FocusTransport::Remote {
            host: host.to_string(),
            marker_id,
        },
        session,
        pane_id,
    )
}

/// Test seam: the active-target probe over the remote (ssh) transport, the
/// twin of [`focus_pane_with`].
#[cfg(test)]
fn active_target_with(
    runner: &dyn CommandRunner,
    host: &str,
    marker_id: u64,
) -> Option<crate::focus::ActiveTarget> {
    crate::focus::active_target_with(
        runner,
        &crate::focus::FocusTransport::Remote {
            host: host.to_string(),
            marker_id,
        },
    )
}

/// Kill a session on the remote tmux server. `(host, name)` uniquely
/// identifies it: `name` is unique within a server (tmux's constraint),
/// `host` picks the server.
pub fn kill_session(host: &str, name: &str) -> Result<(), CommandError> {
    let runner = default_runner();
    let target = shell_single_quote(&exact_target(name));
    run_ssh(
        runner,
        host,
        &[REMOTE_TMUX, "kill-session", "-t", target.as_str()],
    )
    .map(|_| ())
}

/// Rename a session on the remote tmux server. As with `kill_session`,
/// `(host, old_name)` uniquely identifies the target.
pub fn rename_session(host: &str, old_name: &str, new_name: &str) -> Result<(), CommandError> {
    let runner = default_runner();
    // `-t` is the lookup target (exact match); `new_name` is the new label.
    let target = shell_single_quote(&exact_target(old_name));
    let new_name = shell_single_quote(new_name);
    run_ssh(
        runner,
        host,
        &[
            REMOTE_TMUX,
            "rename-session",
            "-t",
            target.as_str(),
            new_name.as_str(),
        ],
    )
    .map(|_| ())
}

/// Persist the display order of `host`'s sessions onto the remote tmux
/// server via the `@deck_order` user option (0-based rank), mirroring local
/// `tmux::persist_session_order`. Survives a deck restart/reconnect as long
/// as the server lives, no config write. `order` lists the session names in
/// new display order. Blocking ssh on an explicit reorder; failures are
/// returned to the session executor for an in-UI warning.
pub fn persist_session_order(host: &str, order: &[String]) -> Result<(), CommandError> {
    persist_session_order_with(default_runner(), host, order)
}

fn persist_session_order_with(
    runner: &dyn CommandRunner,
    host: &str,
    order: &[String],
) -> Result<(), CommandError> {
    if order.is_empty() {
        return Ok(());
    }
    // One ssh hop, one tmux invocation with `;`-separated set-option
    // commands. The remote shell re-parses the joined argv, so the
    // separator is single-quoted (`';'`) to reach tmux as its command
    // separator, not split the shell command; names are quoted likewise.
    let mut argv: Vec<String> = vec![REMOTE_TMUX.to_string()];
    // Bare names, not `exact_target` — see `order_set_option_args`.
    argv.extend(order_set_option_args(order, "';'", shell_single_quote));
    let argv_ref: Vec<&str> = argv.iter().map(String::as_str).collect();
    run_ssh(runner, host, &argv_ref).map(|_| ())
}

/// Create a detached session `name` on the remote tmux server in `dir`
/// (`dir` may contain `~`, expanded by the remote shell). Preserves command
/// failures so the session-control boundary can surface them to the UI.
/// Blocking — runs on an explicit user action.
pub fn new_session(host: &str, name: &str, dir: &str) -> Result<(), CommandError> {
    new_session_with(default_runner(), host, name, dir)
}

fn new_session_with(
    runner: &dyn CommandRunner,
    host: &str,
    name: &str,
    dir: &str,
) -> Result<(), CommandError> {
    let name = shell_single_quote(name);
    let dir = shell_quote_remote_path(dir);
    // Hand the new session the stable agent symlink, or no agent at all — never
    // this call's own `$SSH_AUTH_SOCK`. tmux copies the creating client's
    // environment into the session (`update-environment`), and a one-shot ssh
    // exits within milliseconds, taking its `/tmp/ssh-*/agent.N` with it. That
    // value also *shadows* the global one the attach prelude set, so the first
    // pane of a session created from deck — the one the user is looking at — was
    // left with a dead agent forever. A later attach repairs the session entry
    // but not an already-spawned pane.
    let agent = agent_socket_token();
    let script = format!(
        "if [ -S {agent} ]; then SSH_AUTH_SOCK={agent} ; export SSH_AUTH_SOCK ; \
         else unset SSH_AUTH_SOCK ; fi ; \
         {tmux} new-session -d -s {name} -c {dir}",
        tmux = REMOTE_TMUX,
    );
    run_ssh(runner, host, &[script.as_str()]).map(|_| ())
}

/// List subdirectories under `path` on `host` for the new-session
/// working-dir browser. `path` may contain `~` (remote shell expands it).
/// The returned `Option<String>` is an error message, `None` on success.
///
/// Mirrors local `LocalControl::list_dir`: directories only, sorted,
/// dotfiles included (the picker's pure filter hides them unless the typed
/// leaf starts with `.`). Blocking, but the warm ControlMaster connection
/// makes each call a fast multiplexed hop.
pub fn list_dir(host: &str, path: &str) -> (Vec<String>, Option<String>) {
    list_dir_with(default_runner(), host, path)
}

fn list_dir_with(
    runner: &dyn CommandRunner,
    host: &str,
    path: &str,
) -> (Vec<String>, Option<String>) {
    // `-1` one per line, `-p` suffixes dirs with `/` (keep only those),
    // `-A` includes dotfiles but not `.`/`..`. `--` guards a path starting
    // with `-`. Path is shell-quoted (keeps leading `~`) so spaces /
    // metacharacters stay literal.
    let path = shell_quote_remote_path(path);
    match run_ssh(runner, host, &["ls", "-1pA", "--", path.as_str()]) {
        Ok(raw) => {
            let mut names = parse_dir_listing(&raw);
            names.sort();
            (names, None)
        }
        Err(err) => (Vec::new(), Some(dir_error_message(&err))),
    }
}

/// Keep only directory lines from an `ls -1pA` listing — those `ls -p`
/// suffixed with `/` — and strip the trailing slash. Non-directory lines
/// (no `/`) are dropped.
fn parse_dir_listing(raw: &str) -> Vec<String> {
    raw.lines()
        .filter_map(|line| line.strip_suffix('/'))
        .map(str::to_string)
        .collect()
}

/// Short, one-line error for the picker's error slot. Distinguishes a
/// reachable host whose `ls` failed (missing dir, permission) from one
/// ssh couldn't reach at all.
fn dir_error_message(err: &CommandError) -> String {
    match err {
        CommandError::NonZero { status, stderr, .. } if status.code() != Some(255) => {
            let msg = String::from_utf8_lossy(stderr);
            let msg = msg.to_lowercase();
            if msg.contains("no such file") {
                "not found".to_string()
            } else if msg.contains("permission denied") {
                "permission denied".to_string()
            } else {
                "cannot list directory".to_string()
            }
        }
        _ => "host unreachable".to_string(),
    }
}

/// Directory a staged file lands in on the remote side, as the remote shell
/// spells it. Under `$HOME` rather than `/tmp`: `/tmp` is shared by every user
/// on the host (a fixed name there is another user's to collide with), and a
/// cache directory is the conventional home for bytes Deck can re-send at any
/// time. Emitted double-quoted so `$HOME` expands and a home with a space in it
/// still arrives as one word.
const STAGING_DIR: &str = "\"$HOME/.cache/deck/paste\"";

/// Ceiling on one staged file. Screenshots — the case this exists for — run a
/// few MB; well past that the paste is more likely a mis-drop than an image to
/// show an agent, and the transfer would hold this lane's FIFO worker (see
/// [`UPLOAD_TIMEOUT`]) for the whole upload.
const MAX_STAGED_BYTES: u64 = 20 * 1024 * 1024;

/// Budget for one staged upload. Far beyond [`REMOTE_TIMEOUT`]'s 5s, which
/// sizes a control command: this one streams megabytes, and on a slow link the
/// difference is a working paste rather than a timeout. It is a ceiling on the
/// *stall*, not the expected cost — a screenshot over a warm ControlMaster is
/// well under a second.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(45);

/// Copy `local_path` to the remote id's own filesystem and return the absolute
/// path it landed at, as that side spells it.
///
/// The bytes go over stdin of the same multiplexed ssh connection every other
/// remote call uses — no second authentication, and no argv, which could not
/// carry a megabyte anyway. `Err` is a short user-facing line, like
/// [`list_dir`]'s: it ends up in a warning, not a log.
pub fn upload_file(remote_id: &str, local_path: &std::path::Path) -> Result<String, String> {
    let meta = std::fs::metadata(local_path).map_err(|error| {
        crate::infra::io_error_label(error.kind())
            .unwrap_or("cannot read file")
            .to_string()
    })?;
    if !meta.is_file() {
        return Err("not a file".to_string());
    }
    if meta.len() > MAX_STAGED_BYTES {
        return Err(format!(
            "file is larger than {} MB",
            MAX_STAGED_BYTES / (1024 * 1024)
        ));
    }

    let args = upload_argv(remote_id, &staged_file_name(local_path));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let raw =
        crate::infra::command::run_with_stdin_file("ssh", &arg_refs, local_path, UPLOAD_TIMEOUT)
            .map_err(|error| upload_error_message(&error))?;

    // The remote shell reports the path it wrote, so `$HOME` is expanded by the
    // side that owns it — Deck never has to guess a remote home directory.
    let (path, staged_len) = parse_staged_report(&raw.stdout_trimmed())
        .ok_or_else(|| "host did not report where the file landed".to_string())?;
    // ...and how much of the stream it actually got. Every step of the staging
    // chain exits 0 on a stream that ended early, so the size read back is the
    // only thing that separates a staged image from a path to nothing.
    if staged_len != meta.len() {
        return Err(format!("host stored {staged_len} of {} bytes", meta.len()));
    }
    Ok(path)
}

/// Split the staging command's answer into the path it wrote and the byte count
/// it read back.
///
/// The count is the last whitespace-separated token, and the path everything
/// before it: `wc -c` pads its number on some hosts, and a remote `$HOME` may
/// itself contain a space, so neither can be found by splitting forwards. Only
/// the path's last line is kept, so a login shell that printed a banner ahead of
/// the answer cannot end up inside the path Deck pastes.
fn parse_staged_report(stdout: &str) -> Option<(String, u64)> {
    let (head, size) = stdout.trim_end().rsplit_once(char::is_whitespace)?;
    // `trim_end` before `lines`, or `wc`'s padding *is* the last line and the
    // path looks empty. Interior spaces are left alone: they may be the home's.
    let path = head.trim_end().lines().next_back()?.trim();
    if path.is_empty() {
        return None;
    }
    Some((path.to_string(), size.trim().parse().ok()?))
}

/// Full `ssh` argv for staging one file, mirroring [`run_ssh`]'s assembly so a
/// container id lands inside the container rather than on its host.
///
/// No `PATH` prefix, unlike [`run_ssh`]: every command here (`mkdir`, `cat`,
/// `mv`, `printf`) is a system binary or shell builtin, and a leading
/// assignment would in any case attach only to the first of the four.
fn upload_argv(remote_id: &str, staged_name: &str) -> Vec<String> {
    let target = parse_remote_id(remote_id);
    let mut args = base_ssh_args(target.host);
    args.push(target.host.to_string());
    let command = stage_command(staged_name);
    match target.container {
        None => args.extend(command),
        Some(container) => {
            let refs: Vec<&str> = command.iter().map(String::as_str).collect();
            args.extend(container_exec_argv(
                &container_opts(remote_id).engine,
                container,
                &refs,
                // The one call whose payload is a stream rather than argv.
                ContainerStdin::Attached,
            ));
        }
    }
    args
}

/// The remote-shell command that receives the bytes: create the staging
/// directory, write the stream beside the final name, rename it into place,
/// then print where it went and how much of it arrived.
///
/// The write goes to a `.part` first so a connection that dies mid-transfer
/// leaves no half-image under the name Deck is about to paste — `cat` reports
/// success on a truncated stream, but a broken connection kills the remote
/// shell before the `mv` can run.
///
/// The `wc -c` is the other half of that guarantee, for the stream that ends
/// early without killing anything: a container `exec` that forgot to attach
/// stdin wrote 0 bytes through this exact chain and every step still exited 0.
/// Comparing against the local size is [`upload_file`]'s job — the shell only
/// has to report, which keeps the arithmetic out of a command four different
/// remote shells re-parse.
fn stage_command(staged_name: &str) -> Vec<String> {
    let part = format!(
        "{STAGING_DIR}/{}",
        shell_single_quote(&format!("{staged_name}.part"))
    );
    let final_path = format!("{STAGING_DIR}/{}", shell_single_quote(staged_name));
    vec![
        "mkdir".to_string(),
        "-p".to_string(),
        STAGING_DIR.to_string(),
        "&&".to_string(),
        "cat".to_string(),
        ">".to_string(),
        part.clone(),
        "&&".to_string(),
        "mv".to_string(),
        part,
        final_path.clone(),
        "&&".to_string(),
        "printf".to_string(),
        // Newline-terminated: the byte count follows on its own line, and
        // `parse_staged_report` reads the two apart.
        "'%s\\n'".to_string(),
        final_path.clone(),
        "&&".to_string(),
        "wc".to_string(),
        "-c".to_string(),
        "<".to_string(),
        final_path,
    ]
}

/// Name the file takes on the remote side: a millisecond stamp (two drops of
/// the same screenshot must not collide) plus a sanitized original name.
fn staged_file_name(local_path: &std::path::Path) -> String {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    format!("{stamp}-{}", sanitized_base_name(local_path))
}

/// A remote-safe spelling of the dropped file's name. This string is pasted
/// into an agent's prompt, where a space would end the path — and macOS names
/// its screenshots `Screen Shot 2026-08-17 at 09.41.02.png`, so spaces are the
/// common case, not the exotic one. Anything outside `[A-Za-z0-9._-]` (a CJK
/// screenshot name, a shell metacharacter) becomes `_`; the stamp in
/// [`staged_file_name`] keeps the result unique either way.
fn sanitized_base_name(local_path: &std::path::Path) -> String {
    let raw = local_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (stem, extension) = match raw.rsplit_once('.') {
        Some((stem, extension)) => (stem, extension),
        None => (raw.as_str(), ""),
    };

    // `.` is kept: it needs no quoting anywhere, and dropping it would mangle
    // the timestamps macOS puts in a screenshot's name. It cannot open the
    // result — `staged_file_name` always prefixes a stamp — so no staged file
    // turns into a dotfile.
    let safe = |s: &str, limit: usize| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                    c
                } else {
                    '_'
                }
            })
            .take(limit)
            .collect()
    };

    // The extension is what an agent reads the file type from, so it is kept
    // whole and the stem is what gets truncated.
    let stem = safe(stem, 40);
    let stem = if stem.trim_matches(['_', '.']).is_empty() {
        "image".to_string()
    } else {
        stem
    };
    let extension = safe(extension, 8);
    if extension.is_empty() {
        stem
    } else {
        format!("{stem}.{extension}")
    }
}

/// Short, one-line reason a staged upload failed, for the warning line.
/// Same split as [`dir_error_message`]: a host Deck could not reach reads
/// differently from one whose shell rejected the write.
fn upload_error_message(error: &CommandError) -> String {
    match error {
        CommandError::NonZero { status, stderr, .. } if status.code() != Some(255) => {
            let msg = String::from_utf8_lossy(stderr).to_lowercase();
            if msg.contains("no space left") {
                "no space left on the host".to_string()
            } else if msg.contains("permission denied") {
                "permission denied".to_string()
            } else {
                "host could not store the file".to_string()
            }
        }
        CommandError::Timeout { .. } => "timed out sending the file".to_string(),
        _ => "host unreachable".to_string(),
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/infra/tmux_remote.rs"]
mod tests;
