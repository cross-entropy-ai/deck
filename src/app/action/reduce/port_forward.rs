//! Reducer for the port-forward overlay: the forward list, the add-form
//! field navigation/input, and applying worker task results. Entry point is
//! `reduce_pf`; the rest are form helpers private to this module.

use crate::forwards::ForwardMode;
use crate::state::{
    cycle_option, step_clamped, AppState, PfAddForm, PfField, PortForwardOverlay, SideEffect,
};

use super::PfAction;

pub(super) fn reduce_pf(state: &mut AppState, action: PfAction) -> SideEffect {
    let mut fx = SideEffect::default();
    match action {
        PfAction::Open(host) => {
            state.overlay.context_menu = None;
            state.overlay.port_forward = Some(PortForwardOverlay {
                host,
                selected: 0,
                add_form: None,
                status: None,
            });
        }

        PfAction::Close => {
            state.overlay.port_forward = None;
        }

        PfAction::FocusUp => {
            // Back-step with no upper bound to consult; `saturating_sub` is the
            // whole story, so this stays inline rather than faking a `len`.
            if let Some(o) = state.overlay.port_forward.as_mut() {
                o.selected = o.selected.saturating_sub(1);
            }
        }
        PfAction::FocusDown => {
            let host = state.overlay.port_forward.as_ref().map(|o| o.host.clone());
            if let Some(host) = host {
                let len = forwards_len(state, &host);
                if let Some(o) = state.overlay.port_forward.as_mut() {
                    o.selected = step_clamped(o.selected, len, 1);
                }
            }
        }

        PfAction::AddOpen => {
            if let Some(o) = state.overlay.port_forward.as_mut() {
                o.add_form = Some(PfAddForm::default_for(ForwardMode::Local));
                o.status = None;
            }
        }
        PfAction::AddCancel => {
            if let Some(o) = state.overlay.port_forward.as_mut() {
                o.add_form = None;
            }
        }
        PfAction::AddFieldNext => {
            if let Some(o) = state.overlay.port_forward.as_mut() {
                if let Some(f) = o.add_form.as_mut() {
                    f.focus = next_field(f.focus, f.mode);
                }
            }
        }
        PfAction::AddFieldPrev => {
            if let Some(o) = state.overlay.port_forward.as_mut() {
                if let Some(f) = o.add_form.as_mut() {
                    f.focus = prev_field(f.focus, f.mode);
                }
            }
        }
        PfAction::AddModeLeft => set_mode(&mut state.overlay.port_forward, -1),
        PfAction::AddModeRight => set_mode(&mut state.overlay.port_forward, 1),
        PfAction::AddInputKey(key) => {
            if let Some(o) = state.overlay.port_forward.as_mut() {
                if let Some(f) = o.add_form.as_mut() {
                    handle_pf_input(f, key);
                }
            }
        }

        // These two stay no-ops here; the side effect that actually contacts
        // the worker is dispatched in `dispatch.rs`.
        PfAction::AddSubmit | PfAction::Delete => {}

        PfAction::TaskResult {
            host,
            op,
            ok,
            message,
        } => {
            fx.merge(apply_pf_task_result(state, &host, &op, ok, &message));
        }

        PfAction::ProbeResult { key, health } => {
            state.forward_health.insert(key, health);
        }
    }
    fx
}

fn forwards_len(state: &AppState, host: &str) -> usize {
    state
        .config_remotes
        .iter()
        .find(|r| r.host == host)
        .map(|r| r.forwards.len())
        .unwrap_or(0)
}

/// Field navigation order for the port-forward add form. Dynamic mode
/// omits the target host/port, so it stops after the listen port.
fn pf_field_order(mode: ForwardMode) -> &'static [PfField] {
    match mode {
        ForwardMode::Dynamic => &[PfField::Mode, PfField::BindAddr, PfField::ListenPort],
        _ => &[
            PfField::Mode,
            PfField::BindAddr,
            PfField::ListenPort,
            PfField::TargetHost,
            PfField::TargetPort,
        ],
    }
}

fn next_field(f: PfField, mode: ForwardMode) -> PfField {
    cycle_option(pf_field_order(mode), f, 1)
}

fn prev_field(f: PfField, mode: ForwardMode) -> PfField {
    cycle_option(pf_field_order(mode), f, -1)
}

fn set_mode(o: &mut Option<PortForwardOverlay>, delta: i32) {
    if let Some(o) = o.as_mut() {
        if let Some(f) = o.add_form.as_mut() {
            let modes = [
                ForwardMode::Local,
                ForwardMode::Remote,
                ForwardMode::Dynamic,
            ];
            f.mode = cycle_option(&modes, f.mode, delta);
            if matches!(f.mode, ForwardMode::Dynamic)
                && matches!(f.focus, PfField::TargetHost | PfField::TargetPort)
            {
                f.focus = PfField::ListenPort;
            }
        }
    }
}

/// Feed a key event to the focused field. Filters non-digit input on port
/// fields and whitespace on every field, and rolls back any input that would
/// push a port outside `u16` range.
fn handle_pf_input(f: &mut PfAddForm, key: crossterm::event::KeyEvent) {
    use crossterm::event::KeyCode;
    let port_field = matches!(f.focus, PfField::ListenPort | PfField::TargetPort);
    if let KeyCode::Char(c) = key.code {
        if port_field && !c.is_ascii_digit() {
            return;
        }
        // Whitespace is never valid in any field — it'd just get trimmed
        // on save anyway. Block at input so the value the user sees is
        // the value that's persisted.
        if c.is_whitespace() {
            return;
        }
    }
    let Some(ta) = f.focused_textarea_mut() else {
        return;
    };
    let snapshot = ta.clone();
    ta.input(key);

    // Rollback if a port field was driven outside `u16` by this keystroke.
    // Empty is fine (in-progress typing); anything that parses as u16
    // (0–65535) is fine; everything else (e.g., "99999") is rejected.
    if port_field {
        let s = f.field_text(f.focus);
        if !s.is_empty() && s.parse::<u16>().is_err() {
            if let Some(ta) = f.focused_textarea_mut() {
                *ta = snapshot;
            }
        }
    }
}

/// Turn raw ssh stderr from a failed `-O forward` into a short, plain-language
/// reason. Falls back to ssh's own words (minus noisy prefixes) for cases we
/// don't recognize, so nothing is ever silently swallowed.
fn humanize_forward_error(raw: &str) -> String {
    let lc = raw.to_ascii_lowercase();
    if lc.contains("address already in use") {
        "That local port is already in use on this machine.".into()
    } else if lc.contains("remote port forwarding failed") {
        "The host refused it — that port may already be in use there.".into()
    } else if lc.contains("administratively prohibited") || lc.contains("open failed") {
        "The server blocked forwarding (check its AllowTcpForwarding setting).".into()
    } else if lc.contains("permission denied") {
        "The host denied the connection (permission denied).".into()
    } else if lc.contains("connection refused") {
        "Connection refused by the target.".into()
    } else if lc.contains("could not resolve") || lc.contains("name or service not known") {
        "Couldn't resolve the target host name.".into()
    } else if lc.contains("timed out") || lc.contains("timeout") {
        "The host didn't respond in time (timed out).".into()
    } else if lc.contains("port forwarding failed")
        || lc.contains("forward request failed")
        || lc.contains("mux_client_forward")
    {
        // ssh's ControlMaster mux path reports this when `-O forward` is
        // rejected — almost always because the listen port is already taken.
        "Couldn't set up the forward — the listen port may already be in use.".into()
    } else {
        let cleaned = raw.trim().trim_start_matches("Warning: ").trim();
        if cleaned.is_empty() {
            "ssh rejected the forward.".into()
        } else {
            format!("Couldn't add the forward: {cleaned}")
        }
    }
}

/// Finalize an in-flight `AddForward` (lazy persist: only on worker success).
/// On success: append to `config_remotes`, request config save, close the form.
/// On failure: keep the form open, clear `submitting`, set the error status.
fn apply_pf_task_result(
    state: &mut AppState,
    host: &str,
    op: &crate::app::ssh::port_forward_task::OpKind,
    ok: bool,
    message: &str,
) -> SideEffect {
    use crate::app::ssh::port_forward_task::OpKind;
    let mut fx = SideEffect::default();

    // --- Side effects independent of overlay state ---
    match op {
        OpKind::Forward(_, spec) if ok => {
            if let Some(r) = state.config_remotes.iter_mut().find(|r| r.host == host) {
                if !r.forwards.contains(spec) {
                    r.forwards.push(spec.clone());
                }
            }
            fx.save_config();
        }
        OpKind::Master(_) if !ok => {
            for entry in state.entries.iter_mut() {
                if entry.host.as_deref() == Some(host) {
                    entry.kind = crate::state::SessionEntryKind::Unreachable;
                }
            }
        }
        _ => {}
    }

    // --- Overlay UI updates (gated on overlay being open for this host) ---
    let Some(overlay) = state.overlay.port_forward.as_mut() else {
        return fx;
    };
    if overlay.host != host {
        return fx;
    }
    match op {
        OpKind::Forward(_, _) => {
            if ok {
                overlay.add_form = None;
                overlay.status = Some("Forward added.".into());
            } else {
                if let Some(f) = overlay.add_form.as_mut() {
                    f.submitting = false;
                }
                overlay.status = Some(humanize_forward_error(message));
            }
        }
        OpKind::Cancel(_) => {
            overlay.status = Some(if ok {
                "forward cancelled".into()
            } else {
                format!("warn: cancel failed ({})", message)
            });
        }
        OpKind::Master(_) => {
            if !ok {
                overlay.status = Some(format!("master: {}", message));
            }
        }
        OpKind::Exit(_) => {
            if !ok {
                overlay.status = Some(format!("exit: {}", message));
            }
        }
        // Unreachable in practice: probe results are dispatched as
        // PfAction::ProbeResult and never reach this function. Arm kept for
        // match exhaustiveness over &OpKind.
        OpKind::Probe(_, _) => {}
    }
    fx
}
