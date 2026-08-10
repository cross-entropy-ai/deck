//! The `Effect` enum and `SideEffect` collector — "what the reducer wants
//! done" — plus the request DTOs they carry. Reducers stay IO-free by pushing
//! `Effect`s; `app::dispatch` iterates them in order and does the tmux/ssh/PTY work.

use crate::geometry::AgentTarget;
use crate::geometry::LaneActionAnchor;
use crate::lane::LaneId;
use crate::model::session::SessionId;

#[derive(Debug)]
pub enum Effect {
    /// Activate a live session through its lane-qualified identity. Attachment
    /// transport selection stays below the reducer/effect protocol.
    ActivateSession(SessionId),
    /// Focus a detected agent's pane (Agents tab Enter / number jump).
    /// App's dispatch layer routes this exactly like an agent-row click.
    SwitchAgentPane(AgentTarget),
    /// Show a synthetic lane row that has no attachable session.
    ShowLanePlaceholder(LaneId),
    KillSession(KillRequest),
    RenameSession(RenameRequest),
    /// Create a session with `req.name` at `req.dir`.
    CreateSession(CreateSessionRequest),
    /// Remove a configured non-primary lane from the shell and its backend.
    RemoveLane(LaneId),
    /// Return a backend-owned lane action to its provider. The provider maps
    /// the typed id to a small generic shell intent; App never interprets the
    /// system id or action id.
    InvokeLaneAction {
        lane: LaneId,
        action: crate::system::LaneActionId,
        anchor: LaneActionAnchor,
    },
    OpenPortForwardOverlay(LaneId),
    OpenConfiguredPortForwards,
    OpenNewSessionPicker(LaneId),
    OpenAddRemotePicker,
    AddConfiguredLane {
        owner: crate::system::SystemId,
        candidate: String,
    },
    RereadNewSessionEntries,
    ResizePty {
        /// Clear the host terminal before the next draw after resize.
        full_redraw: bool,
    },
    SaveConfig,
    SaveSessionOrder(LaneId),
    ApplyTmuxTheme,
    /// Ask the host terminal which color scheme it is showing (`CSI ? 996 n`),
    /// so "follow terminal" theme mode picks the matching dark/light theme. The
    /// answer arrives asynchronously as a `ColorScheme` event.
    QueryColorScheme,
    RefreshSessions,
    Quit,
}

#[derive(Debug, Default)]
pub struct SideEffect {
    effects: Vec<Effect>,
}

impl SideEffect {
    pub fn effects(&self) -> &[Effect] {
        &self.effects
    }

    pub fn merge(&mut self, other: SideEffect) {
        self.effects.extend(other.effects);
    }

    pub fn push(&mut self, effect: Effect) {
        self.effects.push(effect);
    }

    pub fn save_config(&mut self) {
        self.push(Effect::SaveConfig);
    }

    pub fn refresh_sessions(&mut self) {
        self.push(Effect::RefreshSessions);
    }

    pub fn resize_pty(&mut self, full_redraw: bool) {
        self.push(Effect::ResizePty { full_redraw });
    }

    pub fn reread_new_session_entries(&mut self) {
        self.push(Effect::RereadNewSessionEntries);
    }

    pub fn has_quit(&self) -> bool {
        self.effects
            .iter()
            .any(|effect| matches!(effect, Effect::Quit))
    }
}

/// Test-only accessors returning the first matching variant's payload:
/// `=> &str` as `&str`, `=> &Ty` by reference.
#[cfg(test)]
macro_rules! effect_finders {
    ($( $name:ident : $variant:ident => &str );* $(;)?) => {
        $(
            pub fn $name(&self) -> Option<&str> {
                self.effects.iter().find_map(|effect| match effect {
                    Effect::$variant(v) => Some(v.as_str()),
                    _ => None,
                })
            }
        )*
    };
    ($( $name:ident : $variant:ident => &$ty:ty );* $(;)?) => {
        $(
            pub fn $name(&self) -> Option<&$ty> {
                self.effects.iter().find_map(|effect| match effect {
                    Effect::$variant(v) => Some(v),
                    _ => None,
                })
            }
        )*
    };
}

/// Test-only `bool` predicates: each row maps a method to an `Effect`
/// pattern checked with `matches!`.
#[cfg(test)]
macro_rules! effect_predicates {
    ($( $name:ident => $pat:pat ),* $(,)?) => {
        $(
            pub fn $name(&self) -> bool {
                self.effects.iter().any(|effect| matches!(effect, $pat))
            }
        )*
    };
}

#[cfg(test)]
impl SideEffect {
    effect_finders! {
        first_kill_session: KillSession => &KillRequest;
        first_rename_session: RenameSession => &RenameRequest;
    }

    pub fn first_activated_session(&self) -> Option<&SessionId> {
        self.effects.iter().find_map(|effect| match effect {
            Effect::ActivateSession(id) => Some(id),
            _ => None,
        })
    }

    pub fn first_lane_placeholder(&self) -> Option<&LaneId> {
        self.effects.iter().find_map(|effect| match effect {
            Effect::ShowLanePlaceholder(lane) => Some(lane),
            _ => None,
        })
    }

    pub fn first_saved_session_order(&self) -> Option<&LaneId> {
        self.effects.iter().find_map(|effect| match effect {
            Effect::SaveSessionOrder(lane) => Some(lane),
            _ => None,
        })
    }

    pub fn first_removed_lane(&self) -> Option<&LaneId> {
        self.effects.iter().find_map(|effect| match effect {
            Effect::RemoveLane(lane) => Some(lane),
            _ => None,
        })
    }

    effect_predicates! {
        has_open_new_session_picker => Effect::OpenNewSessionPicker(_),
        has_resize_pty => Effect::ResizePty { .. },
        has_full_redraw_after_resize => Effect::ResizePty { full_redraw: true },
        has_save_config => Effect::SaveConfig,
        has_save_session_order => Effect::SaveSessionOrder(_),
        has_refresh_sessions => Effect::RefreshSessions,
        has_reread_new_session_entries => Effect::RereadNewSessionEntries,
    }
}

/// Info needed to execute a kill: which session to kill, and optionally
/// which session to switch to first (if killing the current session).
#[derive(Debug)]
pub struct KillRequest {
    pub name: String,
    /// Exact mounted backend lane that owns the session.
    pub lane: LaneId,
    /// Same-lane session to switch to before killing the displayed target.
    /// The executor applies this only when that lane is active, so closing a
    /// session on another host never yanks the current view.
    pub switch_to: Option<String>,
}

/// Info needed to execute a rename.
#[derive(Debug)]
pub struct RenameRequest {
    pub old_name: String,
    pub new_name: String,
    /// Exact mounted backend lane that owns the session.
    pub lane: LaneId,
}

/// Info needed to execute "create a new tmux session".
#[derive(Debug)]
pub struct CreateSessionRequest {
    pub name: String,
    pub dir: String,
    /// Exact mounted backend lane on which to create the session.
    pub lane: LaneId,
}
