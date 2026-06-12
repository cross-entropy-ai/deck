//! The `Effect` enum and `SideEffect` collector — deck's app-level
//! vocabulary for "what the reducer wants done" — plus the request DTOs the
//! effects carry. Reducers stay IO-free by pushing `Effect`s; `app::dispatch`
//! iterates them in order and performs the actual tmux/ssh/PTY work.

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
    RefreshSessions,
    Quit,
}

#[derive(Debug, Default)]
pub struct SideEffect {
    effects: Vec<Effect>,
}

/// Generate `SideEffect` push-helpers from a `method(args) => Effect`
/// table; each body is `self.push(<effect>)`.
macro_rules! effect_pushers {
    ($(
        $(#[$meta:meta])*
        $name:ident ( $($arg:ident : $ty:ty),* $(,)? ) => $build:expr ;
    )*) => {
        $(
            $(#[$meta])*
            pub fn $name(&mut self, $($arg: $ty),*) {
                self.push($build);
            }
        )*
    };
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

    effect_pushers! {
        switch_session(name: String) => Effect::SwitchSession(name);
        switch_remote(req: RemoteSwitchRequest) => Effect::SwitchRemote(req);
        switch_agent_pane(target: AgentTarget) => Effect::SwitchAgentPane(target);
        show_remote_placeholder(host: String) => Effect::ShowRemotePlaceholder(host);
        kill_session(req: KillRequest) => Effect::KillSession(req);
        rename_session(req: RenameRequest) => Effect::RenameSession(req);
        create_session(req: CreateSessionRequest) => Effect::CreateSession(req);
        remove_remote_host(host: String) => Effect::RemoveRemoteHost(host);
        open_new_session_picker() => Effect::OpenNewSessionPicker;
        open_remote_new_session_picker(host: String) => Effect::OpenRemoteNewSessionPicker(host);
        open_add_remote_picker() => Effect::OpenAddRemotePicker;
        add_remote_host(host: String) => Effect::AddRemoteHost(host);
        reread_new_session_entries() => Effect::RereadNewSessionEntries;
        resize_pty(full_redraw: bool) => Effect::ResizePty { full_redraw };
        save_config() => Effect::SaveConfig;
        save_session_order() => Effect::SaveSessionOrder;
        save_remote_session_order(host: String) => Effect::SaveRemoteSessionOrder(host);
        apply_tmux_theme() => Effect::ApplyTmuxTheme;
        refresh_sessions() => Effect::RefreshSessions;
        quit() => Effect::Quit;
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
    /// LOCAL session to switch to after the kill (only meaningful
    /// when killing the user's currently attached local session).
    /// For remote kills, dispatch returns the user to the local view
    /// instead, and this field is `None`.
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
