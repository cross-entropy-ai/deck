//! Tmux operations against a remote host over SSH.
//!
//! Thin sibling of `infra::tmux`: same parsers and `SessionInfo` shape, but
//! each call shells out to `ssh <host> tmux ...`. Multiplexing options are
//! applied on every invocation so calls reuse one SSH connection even
//! without a `ControlMaster` block in `~/.ssh/config`.

use std::collections::HashMap;
use std::time::Duration;

use crate::agent::DetectedAgent;
use crate::infra::command::{default_runner, CommandError, CommandRunner};
use crate::infra::parser::tmux::{
    exact_target, order_set_option_args, parse_sessions, SESSION_LIST_FORMAT_SSH,
};
use crate::tmux::SessionInfo;

/// Marker separating the pane-pid list from the `ps` snapshot in the
/// combined `agent_probe` ssh call. Must not start with `=` (zsh
/// equals-expansion treats `=word` as a command path) nor `-` (echo flag);
/// plain underscores are safe in any remote shell.
const AGENT_PROBE_MARKER: &str = "__DECK_AGENT_PROBE__";

/// Hard cap on a single remote ssh+tmux call. Generous vs the local 1s
/// budget because the first call may wait for the SSH master to come up.
pub const REMOTE_TIMEOUT: Duration = Duration::from_secs(5);

/// SSH options we apply on *every* remote call. See
/// [`crate::ssh::CONTROL_OPTS`] for why these must be identical across
/// every ssh code path.
pub(crate) fn base_ssh_args() -> Vec<&'static str> {
    crate::ssh::CONTROL_OPTS.to_vec()
}

/// Path prefix prepended to every remote command. SSH's non-interactive
/// shell skips `~/.zshrc` / `~/.profile`, so non-default installs
/// (Homebrew, linuxbrew, per-user) are invisible. Prepending these paths
/// via `PATH=... cmd ...` makes deck work without editing remote startup
/// files. The trailing `$PATH` (expanded by the remote shell) keeps the
/// user's existing path intact.
pub(crate) const REMOTE_PATH_PREFIX: &str =
    "PATH=/opt/homebrew/bin:/usr/local/bin:/home/linuxbrew/.linuxbrew/bin:$PATH";

pub(crate) fn run_ssh(
    runner: &dyn CommandRunner,
    host: &str,
    remote_argv: &[&str],
) -> Result<String, CommandError> {
    let mut args = base_ssh_args();
    args.push(host);
    args.push(REMOTE_PATH_PREFIX);
    args.extend_from_slice(remote_argv);
    runner
        .run("ssh", &args, REMOTE_TIMEOUT)
        .map(|out| out.stdout_trimmed())
}

/// List tmux sessions on `host`.
///
/// - `None` — ssh couldn't reach the host (refused/timeout/auth/DNS, all
///   reported as ssh's own exit 255).
/// - `Some(empty)` — reachable but no tmux server (`list-sessions` exited
///   non-zero with "no server running").
/// - `Some(non-empty)` — the live session list.
pub fn list_sessions(host: &str) -> Option<Vec<SessionInfo>> {
    list_sessions_with(default_runner(), host)
}

fn list_sessions_with(runner: &dyn CommandRunner, host: &str) -> Option<Vec<SessionInfo>> {
    // `$'...'` (bash/zsh ANSI-C quoting) makes the remote shell treat `#`
    // literally (no comment) and `\t` as a splittable tab byte; a
    // POSIX-only shell would need a different escape. Trailing
    // `#{@deck_order}` carries deck's persisted display rank (empty when
    // unset). See `persist_session_order`.
    match run_ssh(
        runner,
        host,
        &["tmux", "list-sessions", "-F", SESSION_LIST_FORMAT_SSH],
    ) {
        // No window-activity probe (unlike local): nothing reads remote
        // activity, so the extra `list-windows -a` roundtrip per host per
        // tick would be waste. Rows parse with `activity = 0`.
        Ok(raw) => Some(parse_sessions(&raw, &HashMap::new())),
        // "no server running" is the *only* failure read as empty: the host
        // is reachable, just sessionless. Other non-zero exits (tmux
        // missing, permission, PATH) and ssh failures stay unreachable.
        Err(err) if is_no_server_error(&err) => Some(Vec::new()),
        Err(_) => None,
    }
}

/// Probe `host` for interactive agents in its tmux panes, in one ssh hop:
/// list panes, then a `ps` snapshot, separated by a marker; the pure
/// `crate::agent::detect_agents` does the rest (same as local, fed over
/// ssh). `None` if unreachable (section stays "probing"); `Some(empty)`
/// for a reachable host with no agents.
pub fn agent_probe(host: &str) -> Option<Vec<DetectedAgent>> {
    let runner = default_runner();
    // Commands joined by a bare `;` (shell separator, run in sequence).
    // `$'…'` protects the `#`/tabs in the tmux format; `2>/dev/null`
    // swallows tmux's "no server" noise so a server-less host still yields
    // a clean ps. Marker must be shell-safe (see `AGENT_PROBE_MARKER`).
    let format = format!(
        "$'{}'",
        crate::infra::parser::pane::PANE_FORMAT.replace('\t', "\\t")
    );
    let raw = run_ssh(
        runner,
        host,
        &[
            "tmux",
            "list-panes",
            "-a",
            "-F",
            &format,
            "2>/dev/null",
            ";",
            "echo",
            AGENT_PROBE_MARKER,
            ";",
            "ps",
            "-axo",
            "pid=,ppid=,args=",
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
    // One remote command: `export PATH` (a `for` loop can't take a leading
    // `PATH=…` assignment, so it goes inside; mirrors `run_ssh`'s prefix so
    // a brew tmux resolves), then loop panes printing a marker line + each
    // buffer.
    let ids = pane_ids
        .iter()
        .map(|p| shell_single_quote(p))
        .collect::<Vec<_>>()
        .join(" ");
    let script = format!(
        "export {prefix}; for p in {ids}; do echo {marker} \"$p\"; \
         tmux capture-pane -p -J -t \"$p\" 2>/dev/null; done",
        prefix = REMOTE_PATH_PREFIX,
        marker = CAPTURE_MARKER,
    );
    let mut args = base_ssh_args();
    args.push(host);
    args.push(&script);
    let Ok(out) = runner.run("ssh", &args, REMOTE_TIMEOUT) else {
        return HashMap::new();
    };
    parse_captures(&out.stdout_trimmed())
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

/// Unquoted glob token matching *all* of this Deck process's marker files
/// for `host` (any `marker_id`). The attach wrapper `rm`s these before
/// writing the fresh one so stale markers don't accumulate. `$HOME` is
/// shell-expanded; the rest is shell-safe (digits + sanitized host); the
/// trailing `*` must stay unquoted to glob.
pub(crate) fn client_marker_glob_token(host: &str) -> String {
    let pid = std::process::id();
    format!(
        "\"$HOME\"/.cache/deck/client-{pid}-{}-*",
        marker_host_part(host)
    )
}

/// The `~/.cache/deck` directory token the attach wrapper `mkdir -p`s
/// before writing the marker file.
pub(crate) fn client_cache_dir_token() -> String {
    shell_quote_remote_path("~/.cache/deck")
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
/// `session`. Fire-and-forget: errors are swallowed because the user
/// will see the failure to switch reflected in the UI anyway.
///
/// The client is targeted explicitly (`-c`) via the tty our attach
/// wrapper recorded for this host — see [`client_marker_token`] — so we
/// don't re-point whatever client tmux happens to consider "current"
/// when more than one client is attached to the same server.
pub fn switch_client(host: &str, marker_id: u64, session: &str) {
    switch_client_with(default_runner(), host, marker_id, session);
}

fn switch_client_with(runner: &dyn CommandRunner, host: &str, marker_id: u64, session: &str) {
    let target = shell_single_quote(&exact_target(session));
    // Switch only when the recorded tty is known, so we target Deck's OWN
    // client. An untargeted `switch-client` could re-point another client,
    // so a missing marker no-ops and a later call (after it lands) switches.
    let cmd = format!(
        "{read_c} ; [ -n \"$C\" ] && tmux switch-client -c \"$C\" -t {target}",
        read_c = read_client_tty(host, marker_id),
    );
    let _ = run_ssh(runner, host, &[cmd.as_str()]);
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

/// Test seam: the active-pane probe over the remote (ssh) transport, the
/// twin of [`focus_pane_with`].
#[cfg(test)]
fn active_pane_with(runner: &dyn CommandRunner, host: &str, marker_id: u64) -> Option<String> {
    crate::focus::active_pane_with(
        runner,
        &crate::focus::FocusTransport::Remote {
            host: host.to_string(),
            marker_id,
        },
    )
}

/// Kill a session on the remote tmux server. `(host, name)` uniquely
/// identifies it: `name` is unique within a server (tmux's constraint),
/// `host` picks the server. Errors are swallowed; the next refresh
/// surfaces the session's continued existence (or absence).
pub fn kill_session(host: &str, name: &str) {
    let runner = default_runner();
    let target = shell_single_quote(&exact_target(name));
    let _ = run_ssh(
        runner,
        host,
        &["tmux", "kill-session", "-t", target.as_str()],
    );
}

/// Rename a session on the remote tmux server. As with `kill_session`,
/// `(host, old_name)` uniquely identifies the target.
pub fn rename_session(host: &str, old_name: &str, new_name: &str) {
    let runner = default_runner();
    // `-t` is the lookup target (exact match); `new_name` is the new label.
    let target = shell_single_quote(&exact_target(old_name));
    let new_name = shell_single_quote(new_name);
    let _ = run_ssh(
        runner,
        host,
        &[
            "tmux",
            "rename-session",
            "-t",
            target.as_str(),
            new_name.as_str(),
        ],
    );
}

/// Persist the display order of `host`'s sessions onto the remote tmux
/// server via the `@deck_order` user option (0-based rank), mirroring local
/// `tmux::persist_session_order`. Survives a deck restart/reconnect as long
/// as the server lives, no config write. `order` lists the session names in
/// new display order. Best-effort, blocking ssh on an explicit reorder.
pub fn persist_session_order(host: &str, order: &[String]) {
    persist_session_order_with(default_runner(), host, order)
}

fn persist_session_order_with(runner: &dyn CommandRunner, host: &str, order: &[String]) {
    if order.is_empty() {
        return;
    }
    // One ssh hop, one tmux invocation with `;`-separated set-option
    // commands. The remote shell re-parses the joined argv, so the
    // separator is single-quoted (`';'`) to reach tmux as its command
    // separator, not split the shell command; names are quoted likewise.
    let mut argv: Vec<String> = vec!["tmux".to_string()];
    argv.extend(order_set_option_args(order, "';'", |name| {
        shell_single_quote(&exact_target(name))
    }));
    let argv_ref: Vec<&str> = argv.iter().map(String::as_str).collect();
    let _ = run_ssh(runner, host, &argv_ref);
}

/// Create a detached session `name` on the remote tmux server in `dir`
/// (`dir` may contain `~`, expanded by the remote shell). Returns whether
/// the create succeeded so the caller can decide whether to switch to it.
/// Blocking — runs on an explicit user action.
pub fn new_session(host: &str, name: &str, dir: &str) -> bool {
    new_session_with(default_runner(), host, name, dir)
}

fn new_session_with(runner: &dyn CommandRunner, host: &str, name: &str, dir: &str) -> bool {
    let name = shell_single_quote(name);
    let dir = shell_quote_remote_path(dir);
    run_ssh(
        runner,
        host,
        &[
            "tmux",
            "new-session",
            "-d",
            "-s",
            name.as_str(),
            "-c",
            dir.as_str(),
        ],
    )
    .is_ok()
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
            let mut names = crate::infra::parser::dir::parse_dir_listing(&raw);
            names.sort();
            (names, None)
        }
        Err(err) => (Vec::new(), Some(dir_error_message(&err))),
    }
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

#[cfg(test)]
#[path = "../../../tests/unit/infra/tmux_remote.rs"]
mod tests;
