//! The `Effect` enum and `SideEffect` collector — "what the reducer wants
//! done" — plus the request DTOs they carry. Reducers stay IO-free by pushing
//! `Effect`s; `app::dispatch` iterates them in order and does the tmux/ssh/PTY work.

use crate::geometry::AgentTarget;

#[derive(Debug)]
pub enum Effect {
    SwitchSession(String),
    /// Switch the main view to a remote session. Carries (host, name)
    /// — App's dispatch layer routes the `tmux switch-client` over ssh.
    SwitchRemote(RemoteSwitchRequest),
    /// Focus a detected agent's pane (Agents tab Enter / number jump).
    /// App's dispatch layer routes this exactly like an agent-row click.
    SwitchAgentPane(AgentTarget),
    /// Show a remote host placeholder in the main pane. Used for
    /// synthetic rows like "(no sessions)" that are focusable but don't
    /// have a tmux session to attach to.
    ShowRemotePlaceholder(String),
    KillSession(KillRequest),
    RenameSession(RenameRequest),
    /// Create a new tmux session with `req.name` at `req.dir`.
    CreateSession(CreateSessionRequest),
    /// Detach a remote host from deck (equivalent to `deck remote remove <host>`).
    RemoveRemoteHost(String),
    /// Reconnect/respawn a remote host's ssh+tmux PTY. Emitted by a System's
    /// `on_button` (the `[⟳]` divider button); App rebuilds the connection.
    ReconnectHost(String),
    /// Open a host's port-forward overlay (the `[⇄N]` badge button).
    OpenForwardOverlay(String),
    /// Open a divider's context menu at `(x, y)`. `host` is `None` for the
    /// `@local` divider, `Some(host)` for a remote one (the `[…]` button).
    OpenDividerMenu {
        host: Option<String>,
        x: u16,
        y: u16,
    },
    OpenNewSessionPicker,
    OpenRemoteNewSessionPicker(String),
    OpenAddRemotePicker,
    AddRemoteHost(String),
    RereadNewSessionEntries,
    ResizePty {
        /// Clear the host terminal before the next draw after resize.
        full_redraw: bool,
    },
    SaveConfig,
    SaveSessionOrder,
    SaveRemoteSessionOrder(String),
    ApplyTmuxTheme,
    /// Re-run the OSC 11 probe of the host terminal's background, so
    /// "follow terminal" theme mode picks the matching dark/light theme.
    ProbeTerminalBg,
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
        first_switch_session: SwitchSession => &str;
        first_remote_placeholder: ShowRemotePlaceholder => &str;
        first_remove_remote_host: RemoveRemoteHost => &str;
        first_save_remote_session_order: SaveRemoteSessionOrder => &str;
        first_open_remote_new_session_picker: OpenRemoteNewSessionPicker => &str;
    }

    effect_finders! {
        first_kill_session: KillSession => &KillRequest;
        first_rename_session: RenameSession => &RenameRequest;
    }

    effect_predicates! {
        has_open_new_session_picker => Effect::OpenNewSessionPicker,
        has_resize_pty => Effect::ResizePty { .. },
        has_full_redraw_after_resize => Effect::ResizePty { full_redraw: true },
        has_save_config => Effect::SaveConfig,
        has_save_session_order => Effect::SaveSessionOrder,
        has_refresh_sessions => Effect::RefreshSessions,
        has_reread_new_session_entries => Effect::RereadNewSessionEntries,
    }
}

/// Info needed to execute a kill: which session to kill, and optionally
/// which session to switch to first (if killing the current session).
#[derive(Debug)]
pub struct KillRequest {
    pub name: String,
    /// `Some(host)` targets the remote tmux server on that host;
    /// `None` targets the local tmux server.
    pub host: Option<String>,
    /// LOCAL session to switch to after the kill (only meaningful when
    /// killing the currently attached local session). For remote kills,
    /// dispatch returns to the local view and this is `None`.
    pub switch_to: Option<String>,
}

/// Info needed to execute a rename.
#[derive(Debug)]
pub struct RenameRequest {
    pub old_name: String,
    pub new_name: String,
    /// `Some(host)` targets the remote tmux server on that host.
    pub host: Option<String>,
}

/// Info needed to execute "create a new tmux session".
#[derive(Debug)]
pub struct CreateSessionRequest {
    pub name: String,
    pub dir: String,
    /// `Some(host)` creates the session on that remote host over ssh;
    /// `None` creates it on the local tmux server.
    pub host: Option<String>,
}

/// Info needed to switch the main view to a remote tmux session.
#[derive(Debug)]
pub struct RemoteSwitchRequest {
    pub host: String,
    pub name: String,
}
