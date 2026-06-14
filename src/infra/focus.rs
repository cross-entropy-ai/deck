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
//! - Bail if we don't know our own client tty (`$C`) — never issue an
//!   untargeted op that could move another attached client.
//! - `select-window`/`select-pane` are *session* state (they move every
//!   client on the session), so only run them when it's safe: either we're
//!   the **sole** client on the session, **or** we're switching between
//!   windows of the session our client is **already on** (there
//!   `switch-client` alone is a no-op, so selecting the window is the only
//!   way to move — and tmux shares one active window across a session's
//!   clients, so a co-client unavoidably follows).
//! - Otherwise just `switch-client -c "$C"` (client-scoped) and report
//!   `SessionOnly`, leaving the session's current window alone.

use std::time::Duration;

use crate::infra::command::{default_runner, CommandRunner};
use crate::infra::parser::tmux::exact_target;
use crate::remote_tmux::{client_marker_token, run_ssh, shell_single_quote};
use crate::tmux::PaneFocus;

/// Echoed by the rule's exact-pane branch (we selected the window/pane).
pub(crate) const FOCUS_EXACT_MARKER: &str = "__DECK_FOCUS_EXACT__";
/// Echoed by the rule's session-only branch (we only re-pointed our client).
pub(crate) const FOCUS_SESSION_MARKER: &str = "__DECK_FOCUS_SESSION__";

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
    // quotes). `bare` is the plain session name to compare against
    // `#{client_session}`; `sess` is the `=`-exact target for tmux `-t`.
    let pane = shell_single_quote(pane_id);
    let bare = shell_single_quote(session);
    let sess = shell_single_quote(&exact_target(session));
    // `;` is single-quoted so tmux (not the shell) reads it as its command
    // separator. `cur` is our client's current session; when it already
    // equals the target, we must select-window (switch-client can't move it).
    format!(
        "{set_client_tty} ; [ -z \"$C\" ] && exit 0 ; \
         cur=$(tmux display-message -t \"$C\" -p '#{{client_session}}' 2>/dev/null) ; \
         if [ \"$cur\" != {bare} ] && \
            tmux list-clients -t {sess} -F '#{{client_tty}}' 2>/dev/null | grep -qvxF \"$C\"; then \
         tmux switch-client -c \"$C\" -t {sess} && echo {session_marker} ; \
         else \
         tmux select-window -t {pane} ';' select-pane -t {pane} ';' switch-client -c \"$C\" -t {sess} && echo {exact_marker} ; \
         fi",
        session_marker = FOCUS_SESSION_MARKER,
        exact_marker = FOCUS_EXACT_MARKER,
    )
}

/// Run the focus rule over the given transport and classify the outcome by
/// the marker the rule echoed: `ExactPane` (window/pane selected),
/// `SessionOnly` (only re-pointed our client), or `Failed` (bailed / the
/// transport errored — caller commits nothing).
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
        Ok(o) if o.contains(FOCUS_SESSION_MARKER) => PaneFocus::SessionOnly,
        _ => PaneFocus::Failed,
    }
}

/// [`run_focus_with`] on the production command runner.
pub(crate) fn run_focus(transport: &FocusTransport, session: &str, pane_id: &str) -> PaneFocus {
    run_focus_with(default_runner(), transport, session, pane_id)
}
