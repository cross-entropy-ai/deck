//! Unified agent-pane focus — one rule, two transports.
//!
//! Switching Deck's view to an agent's pane is the *same* tmux operation
//! whether the session is local or on a remote host; only the transport
//! (local `sh` vs `ssh`) and how we learn Deck's own client tty differ. The
//! decision rule — when to select the exact window/pane vs only re-point our
//! client — lives **once** here, as a shell snippet run by either transport,
//! so the two can't drift (they used to: a same-session fix landed in the
//! local copy and not the remote one).
//!
//! The rule:
//! - Bail if we don't know our own client tty (`$C`) — without it we can't
//!   target our own client, and an untargeted op would be the wrong fix.
//! - Otherwise always `select-window`/`select-pane` the exact pane, then
//!   `switch-client -c "$C"` our own client onto it. Selecting the
//!   window/pane is *session* state, so any co-client on the session is
//!   dragged along too — that's intended: whoever drives deck wins.

use std::time::Duration;

use crate::infra::command::{default_runner, CommandRunner};
use crate::infra::parser::tmux::exact_target;
use crate::remote_tmux::{client_marker_token, run_ssh, shell_single_quote};
use crate::tmux::PaneFocus;

/// Echoed once the rule selected the window/pane and switched our client.
pub(crate) const FOCUS_EXACT_MARKER: &str = "__DECK_FOCUS_EXACT__";

/// Local `sh` focus is near-instant; keep a short ceiling so a wedged tmux
/// can't hang the worker. Remote focus uses ssh's own `REMOTE_TIMEOUT`.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(2);

/// How to reach the tmux server a focus targets, and how to learn Deck's
/// own client tty there — the only things that differ between local and
/// remote. `None`-host vs `Some(host)` at the call site picks the variant;
/// everything past this point is shared.
#[derive(Debug, Clone)]
pub(crate) enum FocusTransport {
    /// Local tmux server. Deck knows its client tty directly (the PTY slave).
    Local { client_tty: String },
    /// Remote tmux over ssh. The client tty is read back from the
    /// per-connection marker file the attach wrapper wrote (`marker_id`
    /// scopes it to the current connection generation).
    Remote { host: String, marker_id: u64 },
}

/// The shared focus rule as a POSIX-sh snippet. `set_client_tty` assigns
/// `$C` (a literal tty locally, a `cat` of the marker file remotely); the
/// rest is identical across transports.
fn focus_command(set_client_tty: &str, session: &str, pane_id: &str) -> String {
    // `pane` is a `%N` id for select-window/select-pane (left bare inside the
    // quotes); `sess` is the `=`-exact target for tmux `-t`.
    let pane = shell_single_quote(pane_id);
    let sess = shell_single_quote(&exact_target(session));
    // `;` is single-quoted so tmux (not the shell) reads it as its command
    // separator. Always select the exact window/pane, then point our own
    // client at it — any co-client on the session follows along.
    format!(
        "{set_client_tty} ; [ -z \"$C\" ] && exit 0 ; \
         tmux select-window -t {pane} ';' select-pane -t {pane} ';' switch-client -c \"$C\" -t {sess} && echo {exact_marker}",
        exact_marker = FOCUS_EXACT_MARKER,
    )
}

/// Run the focus rule over the given transport and classify the outcome by
/// the marker the rule echoed: `ExactPane` (window/pane selected and our
/// client switched), or `Failed` (bailed / the transport errored — caller
/// commits nothing).
pub(crate) fn run_focus_with(
    runner: &dyn CommandRunner,
    transport: &FocusTransport,
    session: &str,
    pane_id: &str,
) -> PaneFocus {
    let out = match transport {
        FocusTransport::Local { client_tty } => {
            let cmd = focus_command(
                &format!("C={}", shell_single_quote(client_tty)),
                session,
                pane_id,
            );
            runner
                .run("sh", &["-c", &cmd], LOCAL_TIMEOUT)
                .map(|o| o.stdout_trimmed())
        }
        FocusTransport::Remote { host, marker_id } => {
            let set_c = format!(
                "C=$(cat {} 2>/dev/null)",
                client_marker_token(host, *marker_id)
            );
            let cmd = focus_command(&set_c, session, pane_id);
            run_ssh(runner, host, &[cmd.as_str()])
        }
    };
    match out {
        Ok(o) if o.contains(FOCUS_EXACT_MARKER) => PaneFocus::ExactPane,
        _ => PaneFocus::Failed,
    }
}

/// [`run_focus_with`] on the production command runner.
pub(crate) fn run_focus(transport: &FocusTransport, session: &str, pane_id: &str) -> PaneFocus {
    run_focus_with(default_runner(), transport, session, pane_id)
}

/// The active pane of Deck's own client over `transport` — the `%N` id of
/// the pane the main view currently mirrors. Same `$C` resolution as the
/// focus rule (literal tty locally, `cat` of the marker remotely); bails to
/// `None` when we don't know our client tty or the query fails. Lets the
/// Agents tab keep its "you are here" marker on whatever pane is really
/// active, even when the user switches panes outside Deck.
pub(crate) fn active_pane_with(
    runner: &dyn CommandRunner,
    transport: &FocusTransport,
) -> Option<String> {
    let out = match transport {
        FocusTransport::Local { client_tty } => {
            let cmd = active_pane_command(&format!("C={}", shell_single_quote(client_tty)));
            runner
                .run("sh", &["-c", &cmd], LOCAL_TIMEOUT)
                .map(|o| o.stdout_trimmed())
        }
        FocusTransport::Remote { host, marker_id } => {
            let set_c = format!(
                "C=$(cat {} 2>/dev/null)",
                client_marker_token(host, *marker_id)
            );
            run_ssh(runner, host, &[active_pane_command(&set_c).as_str()])
        }
    };
    // Only a real `%N` id counts; empty stdout means the script bailed
    // (no client tty), which must not be read as a pane.
    out.ok().map(|o| o.trim().to_string()).filter(|id| {
        id.strip_prefix('%')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// The `$C`-guarded `display-message` that reads our client's active pane.
fn active_pane_command(set_client_tty: &str) -> String {
    format!(
        "{set_client_tty} ; [ -z \"$C\" ] && exit 0 ; \
         tmux display-message -t \"$C\" -p '#{{pane_id}}'",
    )
}

/// [`active_pane_with`] on the production command runner.
pub(crate) fn active_pane(transport: &FocusTransport) -> Option<String> {
    active_pane_with(default_runner(), transport)
}
