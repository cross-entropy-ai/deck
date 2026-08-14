//! Unified agent-pane focus — one rule, two transports.
//!
//! Focusing an agent's pane is the *same* tmux operation local or remote; only
//! the transport (local `sh` vs `ssh`) and how we learn Deck's client tty
//! differ. The decision rule lives **once** here as a shell snippet run by
//! either transport, so the two can't drift.
//!
//! The rule: bail if we don't know our client tty (`$C`), since an untargeted
//! op would be wrong. Otherwise `select-window`/`select-pane` the exact pane,
//! then `switch-client -c "$C"` our client onto it. Selecting the window/pane
//! is *session* state, so any co-client is dragged along too — intended:
//! whoever drives deck wins.

use std::time::Duration;

use crate::infra::command::{default_runner, CommandRunner};
use crate::infra::parser::tmux::exact_target;
use crate::remote_tmux::{run_ssh, shell_single_quote, REMOTE_TMUX};
use crate::tmux::PaneFocus;

/// Echoed once the rule selected the window/pane and switched our client.
pub(crate) const FOCUS_EXACT_MARKER: &str = "__DECK_FOCUS_EXACT__";

/// Local `sh` focus is near-instant; keep a short ceiling so a wedged tmux
/// can't hang the worker. Remote focus uses ssh's own `REMOTE_TIMEOUT`.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(2);

/// How to reach the tmux server and learn Deck's client tty there — the only
/// things that differ local vs remote. `None`-host vs `Some(host)` at the call
/// site picks the variant; everything past this point is shared.
#[derive(Debug, Clone)]
pub(crate) enum FocusTransport {
    /// Local tmux server. Deck knows its client tty directly (the PTY slave).
    Local { client_tty: String },
    /// Remote tmux over ssh. The client tty is read from the per-connection
    /// marker file the attach wrapper wrote (`marker_id` scopes it to the
    /// current connection generation).
    Remote { host: String, marker_id: u64 },
}

/// The shared focus rule as a POSIX-sh snippet. `set_client_tty` assigns
/// `$C` (a literal tty locally, a `cat` of the marker file remotely); the
/// rest is identical across transports.
fn focus_command(set_client_tty: &str, tmux: &str, session: &str, pane_id: &str) -> String {
    // `pane` is a `%N` id for select-window/select-pane (left bare inside the
    // quotes); `sess` is the `=`-exact target for tmux `-t`.
    let pane = shell_single_quote(pane_id);
    let sess = shell_single_quote(&exact_target(session));
    // `;` is single-quoted so tmux (not the shell) reads it as its command
    // separator. Always select the exact window/pane, then point our own
    // client at it — any co-client on the session follows along.
    format!(
        "{set_client_tty} ; [ -z \"$C\" ] && exit 0 ; \
         {tmux} select-window -t {pane} ';' select-pane -t {pane} ';' switch-client -c \"$C\" -t {sess} && echo {exact_marker}",
        exact_marker = FOCUS_EXACT_MARKER,
    )
}

/// Run a `$C`-guarded snippet over the transport, returning its trimmed
/// stdout. `build` gets the `$C` assignment to prefix (a literal tty locally,
/// a `cat` of the marker file remotely) and how to spell `tmux` — the only two
/// things that differ, so the local/remote split stops here.
///
/// Remote invocations take `-u` (see [`REMOTE_TMUX`]): a container's tmux has
/// no locale to infer UTF-8 from, and would replace every byte of a non-ASCII
/// session name in `display-message` output with `_`, leaving the focus rule
/// unable to match the session it just read.
fn run_snippet(
    runner: &dyn CommandRunner,
    transport: &FocusTransport,
    build: impl Fn(&str, &str) -> String,
) -> Result<String, crate::infra::command::CommandError> {
    match transport {
        FocusTransport::Local { client_tty } => {
            let cmd = build(&format!("C={}", shell_single_quote(client_tty)), "tmux");
            runner
                .run("sh", &["-c", &cmd], LOCAL_TIMEOUT)
                .map(|o| o.stdout_trimmed())
        }
        FocusTransport::Remote { host, marker_id } => {
            let set_c = crate::remote_tmux::read_client_tty(host, *marker_id);
            run_ssh(runner, host, &[build(&set_c, REMOTE_TMUX).as_str()])
        }
    }
}

/// Run the focus rule over the transport, classifying by the echoed marker:
/// `ExactPane` (window/pane selected and our client switched), or `Failed`
/// (bailed / transport errored — caller commits nothing).
pub(crate) fn run_focus_with(
    runner: &dyn CommandRunner,
    transport: &FocusTransport,
    session: &str,
    pane_id: &str,
) -> PaneFocus {
    let out = run_snippet(runner, transport, |c, tmux| {
        focus_command(c, tmux, session, pane_id)
    });
    match out {
        Ok(o) if o.contains(FOCUS_EXACT_MARKER) => PaneFocus::ExactPane,
        _ => PaneFocus::Failed,
    }
}

/// [`run_focus_with`] on the production command runner.
pub(crate) fn run_focus(transport: &FocusTransport, session: &str, pane_id: &str) -> PaneFocus {
    run_focus_with(default_runner(), transport, session, pane_id)
}

/// The session and pane Deck's client currently mirrors over `transport`.
/// Reading both from the same `display-message` keeps the Sessions and Agents
/// cursors tied to one backend observation, including switches made inside the
/// embedded tmux client rather than through Deck's sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveTarget {
    pub session: String,
    pub pane_id: String,
}

pub(crate) fn active_target_with(
    runner: &dyn CommandRunner,
    transport: &FocusTransport,
) -> Option<ActiveTarget> {
    let out = run_snippet(runner, transport, active_target_command).ok()?;
    // Pane ids never contain spaces, while tmux session names may. Split only
    // once so the remainder is the exact session name.
    let (pane_id, session) = out.trim_end_matches(['\r', '\n']).split_once(' ')?;
    let pane_valid = pane_id
        .strip_prefix('%')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    (pane_valid && !session.is_empty()).then(|| ActiveTarget {
        session: session.to_string(),
        pane_id: pane_id.to_string(),
    })
}

/// The `$C`-guarded `display-message` that reads our client's active target.
fn active_target_command(set_client_tty: &str, tmux: &str) -> String {
    format!(
        "{set_client_tty} ; [ -z \"$C\" ] && exit 0 ; \
         {tmux} display-message -t \"$C\" -p '#{{pane_id}} #{{session_name}}'",
    )
}

/// [`active_target_with`] on the production command runner.
pub(crate) fn active_target(transport: &FocusTransport) -> Option<ActiveTarget> {
    active_target_with(default_runner(), transport)
}
