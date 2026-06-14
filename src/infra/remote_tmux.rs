//! Tmux operations against a remote host over SSH.
//!
//! This is a thin sibling of `infra::tmux`: same parsers, same shape of
//! `SessionInfo`, but each call shells out to `ssh <host> tmux ...`
//! instead of running tmux locally. Deck self-applies multiplexing
//! options on every invocation so remote calls reuse a single TCP/SSH
//! connection even if the user hasn't put a `ControlMaster` block in
//! `~/.ssh/config` (the `deck remote add` flow strongly suggests they
//! do, but this is the safety net).

use std::collections::HashMap;
use std::time::Duration;

use crate::agent::DetectedAgent;
use crate::infra::command::{default_runner, CommandError, CommandRunner};
use crate::infra::tmux::SessionInfo;
use crate::infra::tmux_parse::{
    exact_target, parse_sessions, DECK_ORDER_OPTION, SESSION_LIST_FORMAT_SSH,
};

/// Marker separating the pane-pid list from the `ps` snapshot in the
/// single combined ssh `agent_probe` runs. Must not start with `=` (zsh
/// equals-expansion would treat it as a command path and eat it) nor with
/// `-` (echo flag); plain underscores are safe in any remote shell.
const AGENT_PROBE_MARKER: &str = "__DECK_AGENT_PROBE__";

/// Hard cap on a single remote ssh+tmux call. Generous compared to the
/// local 1s budget because the first call to a host may have to wait
/// for the SSH master to come up (if we got here before the master
/// finished establishing).
pub const REMOTE_TIMEOUT: Duration = Duration::from_secs(5);

/// SSH options we apply on *every* remote call. See
/// [`crate::ssh::CONTROL_OPTS`] for why these must be identical across
/// every ssh code path.
pub(crate) fn base_ssh_args() -> Vec<&'static str> {
    crate::ssh::CONTROL_OPTS.to_vec()
}

/// Extra path prefix prepended to every remote command. SSH runs
/// commands in a non-interactive shell, which on most setups means
/// only `~/.zshenv` (zsh) or `~/.bashrc` for forced-load configs is
/// sourced — `~/.zshrc` / `~/.profile` are skipped, so anything in a
/// non-default location (Homebrew on macOS, linuxbrew on Linux,
/// per-user installs) is invisible. Prepending these paths via
/// `PATH=... cmd ...` makes deck work out-of-the-box without asking
/// the user to edit remote shell startup files.
///
/// The trailing `$PATH` is expanded by the remote shell, so the
/// user's existing path stays intact and just gets extended.
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
/// - `None` — ssh couldn't reach the host (connection refused, timeout,
///   auth, DNS); ssh reports these as its own exit status 255.
/// - `Some(empty)` — ssh connected but the host has no tmux server up,
///   so `tmux list-sessions` exited non-zero with "no server running".
///   The host is reachable, it just has no sessions.
/// - `Some(non-empty)` — the live session list.
pub fn list_sessions(host: &str) -> Option<Vec<SessionInfo>> {
    list_sessions_with(default_runner(), host)
}

fn list_sessions_with(runner: &dyn CommandRunner, host: &str) -> Option<Vec<SessionInfo>> {
    // Wrap in `$'...'` (bash/zsh ANSI-C quoting) so the remote shell
    // both treats `#` literally (no comment) AND interprets `\t` as a
    // tab byte we can split on. Most remote shells deck talks to are
    // bash/zsh; a POSIX-only remote shell would need a different
    // escape, but that's an acceptable tradeoff today.
    // Trailing `#{@deck_order}` carries deck's persisted display rank
    // (empty when unset). See `persist_session_order`.
    match run_ssh(
        runner,
        host,
        &["tmux", "list-sessions", "-F", SESSION_LIST_FORMAT_SSH],
    ) {
        // No window-activity probe here, unlike the local path: nothing
        // reads remote activity (the most-recently-active attach pick is
        // local-only), so the extra `list-windows -a` ssh roundtrip per host
        // per refresh tick would be pure waste. Rows parse with `activity = 0`.
        Ok(raw) => Some(parse_sessions(&raw, &HashMap::new())),
        // ssh connected and tmux reported there's no server running: the
        // host is reachable, it just has no sessions. This is the *only*
        // failure we read as empty — other non-zero exits (tmux missing,
        // permission, a PATH problem) and ssh connection failures stay
        // unreachable rather than masquerading as "(no sessions)".
        Err(err) if is_no_server_error(&err) => Some(Vec::new()),
        Err(_) => None,
    }
}

/// Probe `host` for interactive agents running in its tmux panes, in one
/// ssh hop: list panes, then a `ps` snapshot, separated by a marker. The
/// pure `crate::agent::detect_agents` does the rest — identical to the
/// local path, just fed over ssh. `None` if the host is unreachable (the
/// section then stays "probing"); `Some(empty)` for a reachable host with
/// no agents (→ `claude 0, codex 0`).
pub fn agent_probe(host: &str) -> Option<Vec<DetectedAgent>> {
    agent_probe_with(default_runner(), host)
}

fn agent_probe_with(runner: &dyn CommandRunner, host: &str) -> Option<Vec<DetectedAgent>> {
    // Two commands joined by a bare `;` (a shell separator, so the remote
    // shell runs them in sequence). `$'…'` protects the `#`/tabs in the
    // tmux format; `2>/dev/null` swallows tmux's "no server" noise so a
    // server-less host still yields a clean ps. The marker must be
    // shell-safe (see `AGENT_PROBE_MARKER`).
    let format = format!("$'{}'", crate::agent::PANE_FORMAT.replace('\t', "\\t"));
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
    let panes = crate::agent::parse_panes(panes_part);
    let mut agents = crate::agent::detect_agents(&panes, ps_part);

    // Classify each agent's status from its pane buffer. One batched hop
    // captures every agent pane at once (the panes are already known from
    // the probe), then the shared classifier runs — same as the local path.
    if !agents.is_empty() {
        let pane_ids: Vec<String> = agents.iter().map(|a| a.pane_id.clone()).collect();
        let buffers = capture_panes_with(runner, host, &pane_ids);
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
pub(crate) fn capture_panes(host: &str, pane_ids: &[String]) -> HashMap<String, String> {
    capture_panes_with(default_runner(), host, pane_ids)
}

/// Capture several remote panes in a SINGLE ssh hop, returning `pane_id ->
/// buffer`. Used to classify remote agents' status without one hop per
/// pane. Pane ids are deck-known `%N` handles. Empty map on failure / no
/// panes.
fn capture_panes_with(
    runner: &dyn CommandRunner,
    host: &str,
    pane_ids: &[String],
) -> HashMap<String, String> {
    if pane_ids.is_empty() {
        return HashMap::new();
    }
    // One remote shell command: export PATH (so a brew-installed tmux
    // resolves, mirroring `run_ssh`'s prefix — a `for` loop can't take a
    // leading `PATH=…` assignment, so it goes inside as `export`), then
    // loop the panes printing a marker line and each buffer.
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
/// ssh reports its *own* failures (refused, timeout, auth, DNS) as exit
/// 255; tmux exits non-zero with a recognizable "no server" message when
/// nothing is running. We require both: a non-255 remote exit AND a
/// stderr that names the missing server.
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
/// token. ssh concatenates the remote argv into a string that the login
/// shell re-parses (argv boundaries are NOT preserved), so any
/// user-supplied name or path must be quoted — both for correctness
/// (spaces) and to keep shell metacharacters from being interpreted.
/// Embedded single quotes are escaped as `'\''`.
pub(crate) fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Quote a remote path, preserving a leading `~` / `~/` as the remote
/// `$HOME`. A single-quoted `~` would NOT expand, and tmux's `-c` won't
/// expand `~` itself, so the home prefix is emitted as `"$HOME"` (the
/// only unquoted part) and the rest is a single-quoted literal.
fn shell_quote_remote_path(path: &str) -> String {
    if path == "~" {
        return "\"$HOME\"".to_string();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return format!("\"$HOME\"/{}", shell_single_quote(rest));
    }
    shell_single_quote(path)
}

/// Remote-shell snippet that reads Deck's recorded client tty for this
/// connection (`host` + `marker_id`) into shell variable `C`. Prefixed to
/// the switch/focus command; both then run tmux **only** when `C` is
/// non-empty and pass it as an explicit `-c "$C"` target — so a missing
/// marker (the reconnect race before the attach wrapper writes it, or an
/// unwritable `~/.cache`) results in no tmux command at all, rather than
/// an untargeted operation that could move another attached client.
/// Writing `-c "$C"` directly (two shell words) avoids the `${C:+…}`
/// zsh word-splitting trap; the guarding `[ -n "$C" ]` is portable.
fn read_client_tty(host: &str, marker_id: u64) -> String {
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
/// wrapper records the tty of *its* tmux client (the `ssh -tt` pty, which
/// is exactly tmux's `#{client_tty}`). One-shot `switch-client` calls read
/// it back and pass it as an explicit `-c` target so they re-point Deck's
/// own client, not whatever tmux treats as current when several clients
/// are attached.
///
/// Keyed by Deck's local pid + host + a per-spawn `marker_id` — the id is
/// what makes it *connection*-scoped, not just process-scoped: each
/// (re)connect allocates a fresh id, so a switch/focus issued during the
/// reconnect race reads the *new* path (absent until the wrapper writes
/// it → empty → safe fallback) and can never pick up the previous
/// connection's stale tty. Lives under `~/.cache` so it's disposable.
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
/// writing the fresh one, so stale markers from prior connections don't
/// accumulate. `$HOME` is expanded by the remote shell; the rest is
/// shell-safe (digits + sanitized host), and the trailing `*` must stay
/// unquoted to glob.
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

/// Confirm — out of band from the PTY stream — that this connection's
/// client-tty marker actually got written, so switch/focus only commit
/// once the `-c` target they need really exists. Returns `true` iff the
/// marker file is present and non-empty.
///
/// Readiness is deliberately NOT inferred from PTY output: that stream can
/// carry shell banners / forced-command noise / arbitrary chunking before
/// (or instead of) any marker, which made stream-sentinel detection both
/// miss real markers and accept absent ones. A dedicated `[ -s marker ]`
/// check answers exactly the question that matters. The brief connect race
/// (the attach prelude writes the marker just after `ssh` connects) is
/// covered by a couple of in-shell retries — the wait happens remotely in
/// one bounded ssh call, so a stalled host is capped by the ssh timeout,
/// not app-side polling.
pub fn wait_for_client_marker(host: &str, marker_id: u64) -> bool {
    wait_for_client_marker_with(default_runner(), host, marker_id)
}

fn wait_for_client_marker_with(runner: &dyn CommandRunner, host: &str, marker_id: u64) -> bool {
    let marker = client_marker_token(host, marker_id);
    // Check immediately, then retry twice at 1s steps (integer `sleep` for
    // POSIX portability). The marker is written ~instantly by the PTY
    // prelude so the first check usually wins; the retries cover the race.
    // Starts with `[` (a simple command) so run_ssh's `PATH=…` prefix
    // attaches cleanly. Total wait stays well under REMOTE_TIMEOUT.
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
    // No-op unless we can target Deck's OWN client: switch only when the
    // recorded tty is known. An untargeted `switch-client` could re-point
    // another attached client, so when the marker is missing we do nothing
    // and let a later call (after the marker lands) switch.
    let cmd = format!(
        "{read_c} ; [ -n \"$C\" ] && tmux switch-client -c \"$C\" -t {target}",
        read_c = read_client_tty(host, marker_id),
    );
    let _ = run_ssh(runner, host, &[cmd.as_str()]);
}

/// Test seam: run the unified focus rule over the remote (ssh) transport.
/// Production focus goes through [`crate::focus::run_focus`]; this thin
/// wrapper exists so the remote-transport tests below can drive the shared
/// rule with a `FakeRunner` and assert the emitted ssh command's shape.
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

/// Kill a session on the remote tmux server. The `(host, name)` tuple
/// uniquely identifies the session — within a single tmux server
/// `name` is unique by tmux's own constraint, and `host` picks the
/// server. Errors are swallowed; the next refresh will surface the
/// session's continued existence (or absence) regardless.
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
/// server via the `@deck_order` user option (0-based rank), mirroring the
/// local `tmux::persist_session_order`. Survives a deck restart/reconnect
/// as long as the remote tmux server lives, with no config write. `order`
/// lists the host's session names in their new display order. Best-effort,
/// blocking ssh — runs on an explicit reorder the user is waiting on.
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
    // separator rather than splitting the shell command; names are
    // single-quoted the same way.
    let mut argv: Vec<String> = vec!["tmux".to_string()];
    for (rank, name) in order.iter().enumerate() {
        if rank > 0 {
            argv.push("';'".to_string());
        }
        argv.push("set-option".to_string());
        argv.push("-t".to_string());
        argv.push(shell_single_quote(&exact_target(name)));
        argv.push(DECK_ORDER_OPTION.to_string());
        argv.push(rank.to_string());
    }
    let argv_ref: Vec<&str> = argv.iter().map(String::as_str).collect();
    let _ = run_ssh(runner, host, &argv_ref);
}

/// Create a detached session `name` on the remote tmux server, starting
/// in `dir`. `dir` may contain `~` (expanded by the remote shell before
/// tmux sees it). Returns whether the create succeeded so the caller can
/// decide whether to switch to it. Blocking — runs on an explicit user
/// action and the caller waits on the result.
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

/// List subdirectories under `path` on `host` for the remote new-session
/// working-dir browser. `path` may contain `~`; the remote shell expands
/// it. The returned `Option<String>` is an error message, `None` on
/// success.
///
/// Mirrors the local `LocalControl::list_dir`: directories only, sorted,
/// with dotfiles included (the picker's pure filter hides them unless the
/// typed leaf starts with `.`). Blocking, but the host's ControlMaster
/// connection is already warm so each call is a fast multiplexed hop.
pub fn list_dir(host: &str, path: &str) -> (Vec<String>, Option<String>) {
    list_dir_with(default_runner(), host, path)
}

fn list_dir_with(
    runner: &dyn CommandRunner,
    host: &str,
    path: &str,
) -> (Vec<String>, Option<String>) {
    // `-1` one per line, `-p` suffixes directories with `/` (so we keep
    // only those), `-A` includes dotfiles but not `.`/`..`. `--` guards a
    // path that begins with `-`. The path is shell-quoted (preserving a
    // leading `~`) so spaces / metacharacters in it stay literal.
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

/// Keep only directory lines — those `ls -p` suffixed with `/` — and
/// strip the trailing slash. Non-directory lines (no `/`) are dropped.
pub(crate) fn parse_dir_listing(raw: &str) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::focus::{FOCUS_EXACT_MARKER, FOCUS_SESSION_MARKER};
    use crate::infra::command::Output;
    use crate::tmux::PaneFocus;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::sync::Mutex;

    fn exit_status(code: i32) -> ExitStatus {
        ExitStatus::from_raw(code << 8)
    }

    /// Hands back the configured result for the `list-sessions` call and
    /// succeeds (empty stdout, unless overridden) for anything else.
    struct FakeRunner {
        list_sessions: Mutex<Option<Result<Output, CommandError>>>,
        /// Joined-arg string of every `run` call, for asserting what was
        /// issued (e.g. the persisted order's set-option chain).
        calls: Mutex<Vec<String>>,
        /// When set, every non-`list-sessions` call fails — simulating an
        /// unreachable host / failed ssh for the focus path.
        fail_others: bool,
        /// Canned stdout for non-`list-sessions` calls — lets a focus test
        /// simulate the remote script echoing its branch marker.
        other_stdout: String,
    }

    impl FakeRunner {
        fn new(list_sessions: Result<Output, CommandError>) -> Self {
            Self {
                list_sessions: Mutex::new(Some(list_sessions)),
                calls: Mutex::new(Vec::new()),
                fail_others: false,
                other_stdout: String::new(),
            }
        }

        /// A runner whose every call fails (ssh can't reach the host).
        fn failing() -> Self {
            Self {
                list_sessions: Mutex::new(None),
                calls: Mutex::new(Vec::new()),
                fail_others: true,
                other_stdout: String::new(),
            }
        }

        /// Canned stdout for non-`list-sessions` calls (e.g. the focus
        /// script's echoed branch marker).
        fn with_other_stdout(mut self, stdout: &str) -> Self {
            self.other_stdout = stdout.to_string();
            self
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _timeout: Duration,
        ) -> Result<Output, CommandError> {
            self.calls.lock().unwrap().push(args.join(" "));
            if self.fail_others && !args.contains(&"list-sessions") {
                return Err(CommandError::Timeout {
                    program: program.to_string(),
                    elapsed: Duration::from_secs(1),
                });
            }
            if args.contains(&"list-sessions") {
                self.list_sessions
                    .lock()
                    .unwrap()
                    .take()
                    .expect("list-sessions should be called once")
            } else {
                Ok(Output {
                    stdout: self.other_stdout.clone().into_bytes(),
                })
            }
        }
    }

    fn ok(stdout: &str) -> Result<Output, CommandError> {
        Ok(Output {
            stdout: stdout.as_bytes().to_vec(),
        })
    }

    #[test]
    fn base_args_include_multiplexing() {
        let args = base_ssh_args();
        let joined = args.join(" ");
        assert!(joined.contains("ControlMaster=auto"));
        assert!(joined.contains("ControlPersist=10m"));
        assert!(joined.contains("BatchMode=yes"));
    }

    #[test]
    fn reachable_host_with_sessions_lists_them() {
        let runner = FakeRunner::new(ok("main\t/home/me"));
        let sessions = list_sessions_with(&runner, "box").expect("reachable host");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "main");
    }

    #[test]
    fn persist_session_order_chains_quoted_set_options_over_ssh() {
        let runner = FakeRunner::new(ok(""));
        persist_session_order_with(&runner, "box", &["a".to_string(), "b".to_string()]);
        let calls = runner.calls();
        assert_eq!(calls.len(), 1, "one ssh hop");
        // Names and the `;` separator are single-quoted so the remote shell
        // passes them literally to tmux (tmux interprets the `;`).
        assert!(
            calls[0]
                .contains("set-option -t '=a' @deck_order 0 ';' set-option -t '=b' @deck_order 1"),
            "got: {}",
            calls[0]
        );
        assert!(calls[0].contains("box"), "targets the host");
    }

    #[test]
    fn persist_session_order_empty_is_noop() {
        let runner = FakeRunner::new(ok(""));
        persist_session_order_with(&runner, "box", &[]);
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn focus_pane_selects_pane_by_stable_id_over_ssh() {
        // Remote echoed the EXACT marker → sole client, exact pane focused.
        let runner = FakeRunner::new(ok("")).with_other_stdout(FOCUS_EXACT_MARKER);
        assert_eq!(
            focus_pane_with(&runner, "box", 7, "work", "%240"),
            PaneFocus::ExactPane,
            "success path"
        );
        let calls = runner.calls();
        assert_eq!(calls.len(), 1, "one ssh hop");
        // Pane id and session single-quoted; `;` quoted so tmux (not the
        // remote shell) treats it as its command separator. The
        // session-global select-window/select-pane only run in the else
        // branch — i.e. when Deck is the sole client on the session — and
        // `switch-client` is always `-c "$C"` scoped to Deck's own client.
        assert!(
            calls[0].contains(
                "select-window -t '%240' ';' select-pane -t '%240' ';' switch-client -c \"$C\" -t '=work'"
            ),
            "pane ids stay bare (already exact); the session target is =-exact: {}",
            calls[0]
        );
        // Reads the per-connection recorded client tty.
        assert!(
            calls[0].contains("C=$(cat") && calls[0].contains(".cache/deck/client-"),
            "reads recorded client tty: {}",
            calls[0]
        );
        // Missing marker bails before any tmux command — never an
        // untargeted op that could move another client.
        assert!(
            calls[0].contains("[ -z \"$C\" ] && exit 0"),
            "bails when the client tty is unknown: {}",
            calls[0]
        );
        // The exact-pane selection is gated on Deck being the only client
        // attached to the session, so a click cannot move a co-attached
        // client's window/pane.
        assert!(
            calls[0].contains("tmux list-clients -t '=work' -F '#{client_tty}'")
                && calls[0].contains("grep -qvxF \"$C\""),
            "guards session-global selects behind a sole-client check: {}",
            calls[0]
        );
    }

    #[test]
    fn switch_client_targets_deck_client_explicitly() {
        // A plain remote session switch must also re-point only Deck's own
        // client — same scoping as focus_pane — and no-op when the marker is
        // missing rather than switch an untargeted client.
        let runner = FakeRunner::new(ok(""));
        switch_client_with(&runner, "box", 7, "work");
        let calls = runner.calls();
        assert_eq!(calls.len(), 1, "one ssh hop");
        assert!(
            calls[0].contains("C=$(cat") && calls[0].contains(".cache/deck/client-"),
            "reads recorded client tty: {}",
            calls[0]
        );
        // Switch runs only when the tty is known, and is always `-c "$C"`
        // scoped. Writing `-c "$C"` directly (two words) avoids the zsh
        // `${C:+…}` single-word-collapse trap.
        assert!(
            calls[0].contains("[ -n \"$C\" ] && tmux switch-client -c \"$C\" -t '=work'"),
            "no-op unless the client tty is known: {}",
            calls[0]
        );
    }

    #[test]
    fn focus_pane_reports_failure_when_ssh_fails() {
        // A failed focus must report `Failed` so the caller won't mark the
        // agent active / switch the view for a focus that never landed.
        let runner = FakeRunner::failing();
        assert_eq!(
            focus_pane_with(&runner, "box", 7, "work", "%240"),
            PaneFocus::Failed
        );
    }

    #[test]
    fn wait_for_client_marker_checks_the_marker_file() {
        // Readiness is confirmed by checking the per-connection marker file
        // out of band (not by parsing the PTY stream). ssh exit 0 → ready.
        let runner = FakeRunner::new(ok(""));
        assert!(wait_for_client_marker_with(&runner, "box", 7));
        let calls = runner.calls();
        assert_eq!(calls.len(), 1, "one ssh hop");
        assert!(
            calls[0].contains("[ -s ") && calls[0].contains(".cache/deck/client-"),
            "checks the marker file's presence: {}",
            calls[0]
        );
    }

    #[test]
    fn wait_for_client_marker_false_when_absent() {
        // ssh non-zero (marker never appeared, or host unreachable) → not
        // ready, so switch/focus keep deferring rather than committing
        // against a marker that was never written.
        let runner = FakeRunner::failing();
        assert!(!wait_for_client_marker_with(&runner, "box", 7));
    }

    #[test]
    fn focus_pane_fails_when_marker_missing() {
        // No recorded client tty (reconnect race / unwritable cache): the
        // remote script bails (`[ -z "$C" ] && exit 0`) before any tmux
        // command and echoes no marker, so focus_pane reports `Failed` and
        // the caller commits nothing — never an untargeted select/switch.
        // Empty stdout models that bail.
        let runner = FakeRunner::new(ok("")).with_other_stdout("");
        assert_eq!(
            focus_pane_with(&runner, "box", 7, "work", "%240"),
            PaneFocus::Failed
        );
    }

    #[test]
    fn focus_pane_reports_session_only_when_co_attached() {
        // When another client shares the session the remote script skips the
        // exact-pane selects and echoes the SESSION marker. focus_pane must
        // surface that as `SessionOnly` so the caller withholds the agent
        // highlight (the main pane may show a different pane).
        let runner = FakeRunner::new(ok("")).with_other_stdout(FOCUS_SESSION_MARKER);
        assert_eq!(
            focus_pane_with(&runner, "box", 7, "work", "%240"),
            PaneFocus::SessionOnly
        );
    }

    #[test]
    fn reachable_host_without_server_reports_no_sessions() {
        // No tmux server up: `tmux list-sessions` exits 1 with
        // "no server running". The host is reachable, so this must be an
        // empty list (→ "(no sessions)"), not unreachable.
        let runner = FakeRunner::new(Err(CommandError::NonZero {
            program: "ssh".to_string(),
            status: exit_status(1),
            stderr: b"no server running on /tmp/tmux-1000/default".to_vec(),
        }));
        let result = list_sessions_with(&runner, "box");
        assert!(result.is_some(), "reachable host should not be None");
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn ssh_connection_failure_is_unreachable() {
        // ssh reports its own connection failures as exit 255.
        let runner = FakeRunner::new(Err(CommandError::NonZero {
            program: "ssh".to_string(),
            status: exit_status(255),
            stderr: b"ssh: connect to host box port 22: Connection refused".to_vec(),
        }));
        assert!(list_sessions_with(&runner, "box").is_none());
    }

    #[test]
    fn tmux_missing_is_unreachable_not_empty() {
        // Reachable host, but tmux isn't installed (127): this is a real
        // error, not "no sessions", so it must not be reported as empty.
        let runner = FakeRunner::new(Err(CommandError::NonZero {
            program: "ssh".to_string(),
            status: exit_status(127),
            stderr: b"bash: tmux: command not found".to_vec(),
        }));
        assert!(list_sessions_with(&runner, "box").is_none());
    }

    #[test]
    fn timeout_is_unreachable() {
        let runner = FakeRunner::new(Err(CommandError::Timeout {
            program: "ssh".to_string(),
            elapsed: Duration::from_secs(5),
        }));
        assert!(list_sessions_with(&runner, "box").is_none());
    }

    /// Returns one canned result for the single ssh call list_dir /
    /// new_session make.
    struct OneShot(Mutex<Option<Result<Output, CommandError>>>);
    impl OneShot {
        fn new(r: Result<Output, CommandError>) -> Self {
            Self(Mutex::new(Some(r)))
        }
    }
    impl CommandRunner for OneShot {
        fn run(&self, _p: &str, _a: &[&str], _t: Duration) -> Result<Output, CommandError> {
            self.0.lock().unwrap().take().expect("called once")
        }
    }

    #[test]
    fn parse_dir_listing_keeps_dirs_drops_files() {
        // `ls -1pA` suffixes directories (incl. dotfile dirs) with `/`.
        let raw = "src/\nmain.rs\ntests/\n.config/\nREADME";
        let mut got = parse_dir_listing(raw);
        got.sort();
        assert_eq!(got, vec![".config", "src", "tests"]);
    }

    #[test]
    fn list_dir_returns_sorted_dirs() {
        let runner = OneShot::new(ok("beta/\nalpha/\nnote.txt"));
        let (dirs, err) = list_dir_with(&runner, "box", "~/");
        assert_eq!(dirs, vec!["alpha", "beta"]);
        assert!(err.is_none());
    }

    #[test]
    fn list_dir_missing_path_reports_not_found() {
        let runner = OneShot::new(Err(CommandError::NonZero {
            program: "ssh".to_string(),
            status: exit_status(2),
            stderr: b"ls: cannot access '~/nope': No such file or directory".to_vec(),
        }));
        let (dirs, err) = list_dir_with(&runner, "box", "~/nope");
        assert!(dirs.is_empty());
        assert_eq!(err.as_deref(), Some("not found"));
    }

    #[test]
    fn list_dir_unreachable_host_reports_unreachable() {
        let runner = OneShot::new(Err(CommandError::NonZero {
            program: "ssh".to_string(),
            status: exit_status(255),
            stderr: b"ssh: connect to host box port 22: Connection refused".to_vec(),
        }));
        let (_dirs, err) = list_dir_with(&runner, "box", "~/");
        assert_eq!(err.as_deref(), Some("host unreachable"));
    }

    #[test]
    fn parse_captures_splits_on_markers() {
        let raw = "__deck_cap__ %1\nline a1\nline a2\n__deck_cap__ %5\nline b1";
        let map = parse_captures(raw);
        assert_eq!(map.get("%1").map(String::as_str), Some("line a1\nline a2"));
        assert_eq!(map.get("%5").map(String::as_str), Some("line b1"));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn parse_captures_handles_empty_buffer() {
        // A pane with no content still gets an (empty) entry.
        let map = parse_captures("__deck_cap__ %2");
        assert_eq!(map.get("%2").map(String::as_str), Some(""));
    }

    #[test]
    fn shell_single_quote_wraps_and_escapes() {
        assert_eq!(shell_single_quote("plain"), "'plain'");
        assert_eq!(shell_single_quote("a b"), "'a b'");
        assert_eq!(shell_single_quote("it's"), "'it'\\''s'");
        // Metacharacters stay inside the quotes — never interpreted.
        assert_eq!(shell_single_quote("a; rm -rf ~"), "'a; rm -rf ~'");
    }

    #[test]
    fn shell_quote_remote_path_keeps_home_expandable() {
        assert_eq!(shell_quote_remote_path("~"), "\"$HOME\"");
        assert_eq!(shell_quote_remote_path("~/"), "\"$HOME\"/''");
        assert_eq!(shell_quote_remote_path("~/proj"), "\"$HOME\"/'proj'");
        assert_eq!(
            shell_quote_remote_path("~/My Docs/a"),
            "\"$HOME\"/'My Docs/a'"
        );
        // Absolute path: fully single-quoted, no expansion.
        assert_eq!(shell_quote_remote_path("/abs/My Docs"), "'/abs/My Docs'");
        // A command-substitution attempt stays a literal directory name.
        assert_eq!(
            shell_quote_remote_path("~/$(reboot)"),
            "\"$HOME\"/'$(reboot)'"
        );
    }

    #[test]
    fn new_session_reports_success_and_failure() {
        let okrunner = OneShot::new(ok(""));
        assert!(new_session_with(&okrunner, "box", "work", "~/proj"));

        let failrunner = OneShot::new(Err(CommandError::Timeout {
            program: "ssh".to_string(),
            elapsed: Duration::from_secs(5),
        }));
        assert!(!new_session_with(&failrunner, "box", "work", "~/proj"));
    }
}
