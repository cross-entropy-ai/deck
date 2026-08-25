//! The `System` extension point: a mounted backend that supplies terminal
//! sessions to the deck shell.
//!
//! deck is a shell that knows nothing about tmux/ssh or "local vs remote". It
//! walks the registered [`System`]s for sections (sidebar structure) and lane
//! snapshots (sessions + agents), and routes control ops and divider-button
//! clicks back through the trait. tmux (local + remote) is one built-in
//! implementation ([`tmux::TmuxSystem`]); a new backend = `impl System` +
//! register, with no shell changes.
//!
//! See `docs/system-trait-design.md`.

pub mod tmux;

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::agent::DetectedAgent;
use crate::config::Config;
use crate::geometry::{LaneActionAnchor, SectionButton};
use crate::lane::LaneId;
use crate::model::session::SessionSnapshot;
use crate::session::SessionControl;

/// Explicit collection of mounted systems. App and background workers receive
/// the same registry reference; model code consumes the resulting
/// [`SectionDef`] values and never performs global backend lookup.
pub struct SystemRegistry<'a> {
    systems: Vec<&'a dyn System>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SystemId(String);

impl SystemId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'a> SystemRegistry<'a> {
    pub fn new(systems: Vec<&'a dyn System>) -> Self {
        Self { systems }
    }

    /// Apply the shell configuration to every system. Each backend extracts
    /// only its own settings and owns them thereafter.
    pub fn configure(&self, config: &Config, remotes: &[crate::config::RemoteConfig]) {
        for system in &self.systems {
            system.configure(config, remotes);
        }
    }

    pub fn config_provider(&self, owner: &SystemId) -> Option<&dyn LaneConfigProvider> {
        let system = self
            .systems
            .iter()
            .copied()
            .find(|system| system.id() == owner.as_str())?;
        system
            .lanes()
            .into_iter()
            .find_map(|lane| system.runtime(&lane)?.lane_config())
    }

    /// Resolve the owner of a lane. Unknown ids stay explicit rather than
    /// falling through to a default backend.
    pub fn runtime(&self, lane: &LaneId) -> Option<LaneRuntime<'a>> {
        self.systems
            .iter()
            .copied()
            .find(|system| system.id() == lane.system())
            .and_then(|system| system.runtime(lane))
    }

    /// Materialize display definitions in registry/lane order. These values
    /// cross into the model; the backend objects do not.
    pub fn sections(&self) -> Vec<SectionDef> {
        self.systems
            .iter()
            .flat_map(|system| {
                system.lanes().into_iter().filter_map(|lane| {
                    let runtime = system.runtime(&lane)?;
                    let mut section = system.section_for(&lane)?;
                    section.session_capabilities = runtime.session_capabilities;
                    section.lane_capabilities = runtime.lane_capabilities;
                    if !runtime.lane_capabilities.actions {
                        section.buttons.clear();
                    }
                    Some(section)
                })
            })
            .collect()
    }

    /// Snapshot routing pairs in registry/lane order. Used by the refresh
    /// worker so adding a System automatically adds its lanes to polling.
    pub fn snapshot_routes(&self) -> Vec<LaneRuntime<'a>> {
        self.systems
            .iter()
            .flat_map(|system| {
                system
                    .lanes()
                    .into_iter()
                    .filter_map(|lane| system.runtime(&lane))
                    .filter(LaneRuntime::has_catalog)
            })
            .collect()
    }
}

static TMUX_SYSTEM: LazyLock<tmux::TmuxSystem> = LazyLock::new(tmux::TmuxSystem::default);
static BUILTIN_SYSTEMS: LazyLock<SystemRegistry<'static>> =
    LazyLock::new(|| SystemRegistry::new(vec![&*TMUX_SYSTEM]));

/// Production composition root. Callers still receive it explicitly, making
/// tests free to construct a registry with a different system set.
pub fn builtin_registry() -> &'static SystemRegistry<'static> {
    &BUILTIN_SYSTEMS
}

/// A mounted backend supplying sessions to the shell. Every method is keyed by
/// [`LaneId`], which already carries the owning system's id — so a system only
/// ever sees lanes it produced in [`section_for`](System::section_for).
pub trait System: Send + Sync {
    /// Stable id for this system (e.g. `"tmux"`). Must match the `system`
    /// half of every [`LaneId`] this system hands out.
    fn id(&self) -> &str;

    /// Refresh backend-owned configuration. The shell passes the neutral app
    /// config plus the lanes it remembers being linked to — the two live in
    /// different files because a user authors one and Deck writes the other
    /// (see `lane_state`), and a backend may need either.
    fn configure(&self, _config: &Config, _remotes: &[crate::config::RemoteConfig]) {}

    /// Configured lanes in display order. Include lanes that currently have no
    /// sessions so the shell can render their empty/loading sections.
    fn lanes(&self) -> Vec<LaneId>;

    /// The [`SectionDef`] for a single lane: the divider's title and buttons.
    /// The shell enumerates the lanes to lay out from its session list (so
    /// every session row keeps a section) and calls this to style each one.
    fn section_for(&self, lane: &LaneId) -> Option<SectionDef>;

    /// Compose only the capabilities this lane actually supports. A
    /// catalog-only backend returns a runtime without control/action ports.
    fn runtime(&self, lane: &LaneId) -> Option<LaneRuntime<'_>>;
}

pub trait SessionCatalog: Send + Sync {
    /// Snapshot one lane's sessions + detected agents. Run off the UI thread by
    /// the refresh worker. Reachability and backend failures remain distinct
    /// typed errors; a successful empty snapshot means the lane is reachable
    /// but has no sessions. `probe_agents` is the shell's hint that the Agents
    /// tab is active — when false a backend should
    /// skip the (possibly expensive) agent detection and leave
    /// [`LaneSnapshot::agents`] `None`.
    fn snapshot(&self, lane: &LaneId, ctx: &SnapshotCtx<'_>) -> Result<LaneSnapshot, CatalogError>;

    /// Whether a lane should be sampled inline with the coalesced refresh
    /// worker or on the guarded parallel background path.
    fn snapshot_mode(&self, _lane: &LaneId) -> SnapshotMode {
        SnapshotMode::Background
    }
}

pub trait SessionControlProvider: Send + Sync {
    /// The control-plane handle for one lane (switch/rename/kill/create/…),
    /// run on the executor's per-lane worker thread. `ctx` carries the shell
    /// runtime state a backend may need to construct it (e.g. tmux reads the
    /// local client tty and a remote's reconnect marker id).
    fn control(&self, lane: &LaneId, ctx: &ControlCtx) -> Box<dyn SessionControl + Send>;
}

pub trait LaneActionProvider: Send + Sync {
    /// Handle a typed action this system declared on `lane`'s divider. The
    /// anchor is the button's screen position for positioned UI. Returns only
    /// generic shell intents, so neither reducer nor App decodes backend ids.
    fn invoke(
        &self,
        lane: &LaneId,
        action: &LaneActionId,
        anchor: LaneActionAnchor,
    ) -> Vec<LaneShellIntent>;
}

/// A lane a system could mount on demand, found at runtime rather than declared
/// in configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountCandidate {
    /// Backend-owned identity. The shell stores it and hands it back verbatim;
    /// only the owning system decodes it, exactly like [`LaneActionId`].
    pub id: String,
    /// What the picker shows.
    pub label: String,
    /// Whether mounting has to change something outside Deck first (starting a
    /// stopped container, say). The shell confirms with the user before it
    /// commits to that, and never for candidates that don't need it.
    pub needs_activation: bool,
}

/// Lanes a system can discover and mount for the current session only. Distinct
/// from [`LaneConfigProvider`], which persists a lane into the config file:
/// nothing here survives a restart, so the shell must not write these to disk.
pub trait LaneMountProvider: Send + Sync {
    /// What `lane` could mount right now. **Blocking** — the shell runs it on a
    /// worker thread, like [`SessionCatalog::snapshot`]. The error is shown to
    /// the user verbatim, so it should read as a reason, not a stack.
    fn discover(&self, lane: &LaneId) -> Result<Vec<MountCandidate>, String>;

    /// Bring a candidate into a mountable state. **Blocking**, and only called
    /// for a candidate that declared `needs_activation` after the user agreed.
    fn activate(&self, lane: &LaneId, candidate: &str) -> Result<(), String>;

    /// Record the candidate as a linked lane and return its id, writing it
    /// into the lane set the caller then persists.
    ///
    /// It used to be session-scoped, on the grounds that a mount must not be
    /// written to the config file. That was right about the config file and
    /// wrong about persistence: the lane set is what Deck remembers, and it
    /// lives in its own file now (see `lane_state`), so a container mounted
    /// today is there again tomorrow.
    fn mount(
        &self,
        lane: &LaneId,
        candidate: &str,
        remotes: &mut Vec<crate::config::RemoteConfig>,
    ) -> Option<LaneId>;
}

/// Backend-owned configuration mutations addressed by lane identity.
/// The shell supplies the persisted configuration as a whole and applies the
/// typed outcome; only the owning backend interprets its lane representation.
/// Add/remove a lane from the set Deck remembers being linked to. The list is
/// passed rather than the config, because the lane set moved out of the config
/// file — a linked host is something Deck recorded, not something a user
/// configured (see `lane_state`).
pub trait LaneConfigProvider: Send + Sync {
    fn add_lane(
        &self,
        candidate: &str,
        remotes: &mut Vec<crate::config::RemoteConfig>,
    ) -> LaneConfigAddOutcome;
    fn remove_lane(
        &self,
        lane: &LaneId,
        remotes: &mut Vec<crate::config::RemoteConfig>,
    ) -> LaneConfigOutcome;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneConfigAddOutcome {
    Added(LaneId),
    AlreadyExists,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneConfigOutcome {
    Removed,
    Unsupported,
}

/// Attachment-owned connection facts supplied transiently while a backend
/// constructs an operational focus transport. This is not persisted lane
/// metadata and cannot be used as a connection lookup key.
pub(crate) enum AttachmentEndpoint<'a> {
    Primary { client_locator: &'a str },
    Managed { marker_id: u64 },
}

pub(crate) trait FocusTransportProvider: Send + Sync {
    fn focus_transport(
        &self,
        lane: &LaneId,
        endpoint: AttachmentEndpoint<'_>,
    ) -> Option<crate::focus::FocusTransport>;
}

pub(crate) trait SummaryTransportProvider: Send + Sync {
    fn summary_pane(
        &self,
        lane: &LaneId,
        id: String,
        target: String,
    ) -> Option<crate::summary::SummaryPane>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentRole {
    Primary,
    Managed,
}

/// Declares that a lane owns a terminal attachment and how the shell mounts
/// it. Connection details remain inside the backend/attachment adapter.
pub trait AttachmentProvider: Send + Sync {
    fn role(&self, lane: &LaneId) -> Option<AttachmentRole>;
}

/// Backend-owned identifier for a lane action. The shell stores and returns
/// this value without decoding string commands or matching system ids.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LaneActionId(String);

impl LaneActionId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for LaneActionId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// Small shell vocabulary produced after a backend interprets its own action
/// id. App may execute these intents without knowing which system supplied
/// them or what identifier selected them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneShellIntent {
    ReconnectAttachment,
    OpenPortForwards,
    OpenContextMenu { anchor: LaneActionAnchor },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionCapabilities {
    pub activate: bool,
    pub rename: bool,
    pub kill: bool,
}

/// Where a forward on a lane points — and therefore what the form has to ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ForwardEndpointKind {
    /// The user names the endpoint. Every ssh mode is available and the target
    /// host and port are typed in: the lane is the *route*, not the destination.
    #[default]
    Explicit,
    /// The lane itself is the endpoint, and its address is the owning system's
    /// to resolve. Only a local forward means anything — `-R` and `-D` put the
    /// listener on the far side and the destination somewhere else entirely, so
    /// neither one would reach this lane at all — and the user is asked for a
    /// port, never an address: a container's changes when it restarts.
    Lane,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LaneCapabilities {
    pub create_session: bool,
    pub reorder_sessions: bool,
    pub actions: bool,
    /// What a forward on this lane points at. Only meaningful while
    /// [`port_forwards`](Self::port_forwards) is set.
    pub forward_endpoint: ForwardEndpointKind,
    /// Whether this lane can carry port forwards right now. The owning system
    /// decides — for tmux that means the lane has a connection of its own AND
    /// Deck's connection reuse is on, since the forward commands are `ssh -O`
    /// against that shared socket. The shell greys its forward affordances out
    /// from this flag instead of re-deriving a backend's answer.
    pub port_forwards: bool,
    /// Whether this lane can discover further lanes to mount — see
    /// [`LaneMountProvider`]. The shell offers the picker only where true.
    pub mounts: bool,
}

#[derive(Clone)]
pub struct LaneRuntime<'a> {
    lane: LaneId,
    catalog: Option<&'a dyn SessionCatalog>,
    session_control: Option<&'a dyn SessionControlProvider>,
    lane_actions: Option<&'a dyn LaneActionProvider>,
    lane_config: Option<&'a dyn LaneConfigProvider>,
    lane_mounts: Option<&'a dyn LaneMountProvider>,
    focus_transport: Option<&'a dyn FocusTransportProvider>,
    summary_transport: Option<&'a dyn SummaryTransportProvider>,
    attachment: Option<&'a dyn AttachmentProvider>,
    pub session_capabilities: SessionCapabilities,
    pub lane_capabilities: LaneCapabilities,
}

impl<'a> LaneRuntime<'a> {
    pub fn new(lane: &LaneId) -> Self {
        Self {
            lane: lane.clone(),
            catalog: None,
            session_control: None,
            lane_actions: None,
            lane_config: None,
            lane_mounts: None,
            focus_transport: None,
            summary_transport: None,
            attachment: None,
            session_capabilities: SessionCapabilities::default(),
            lane_capabilities: LaneCapabilities::default(),
        }
    }

    pub fn with_catalog(mut self, catalog: &'a dyn SessionCatalog) -> Self {
        self.catalog = Some(catalog);
        self
    }

    pub fn with_session_control(mut self, control: &'a dyn SessionControlProvider) -> Self {
        self.session_control = Some(control);
        self
    }

    pub fn with_lane_actions(mut self, actions: &'a dyn LaneActionProvider) -> Self {
        self.lane_actions = Some(actions);
        self
    }

    pub fn with_lane_config(mut self, config: &'a dyn LaneConfigProvider) -> Self {
        self.lane_config = Some(config);
        self
    }

    pub fn with_lane_mounts(mut self, mounts: &'a dyn LaneMountProvider) -> Self {
        self.lane_mounts = Some(mounts);
        self
    }

    pub(crate) fn with_focus_transport(mut self, provider: &'a dyn FocusTransportProvider) -> Self {
        self.focus_transport = Some(provider);
        self
    }

    pub(crate) fn with_summary_transport(
        mut self,
        provider: &'a dyn SummaryTransportProvider,
    ) -> Self {
        self.summary_transport = Some(provider);
        self
    }

    pub fn with_attachment(mut self, provider: &'a dyn AttachmentProvider) -> Self {
        self.attachment = Some(provider);
        self
    }

    pub fn with_capabilities(
        mut self,
        session: SessionCapabilities,
        lane: LaneCapabilities,
    ) -> Self {
        self.session_capabilities = session;
        self.lane_capabilities = lane;
        self
    }

    pub fn lane(&self) -> &LaneId {
        &self.lane
    }

    pub fn has_catalog(&self) -> bool {
        self.catalog.is_some()
    }

    pub fn catalog(&self) -> Option<&'a dyn SessionCatalog> {
        self.catalog
    }

    pub fn session_control(&self) -> Option<&'a dyn SessionControlProvider> {
        self.session_control
    }

    pub fn lane_actions(&self) -> Option<&'a dyn LaneActionProvider> {
        self.lane_actions
    }

    pub fn lane_config(&self) -> Option<&'a dyn LaneConfigProvider> {
        self.lane_config
    }

    pub fn lane_mounts(&self) -> Option<&'a dyn LaneMountProvider> {
        self.lane_mounts
    }

    pub(crate) fn focus_transport(&self) -> Option<&'a dyn FocusTransportProvider> {
        self.focus_transport
    }

    pub(crate) fn summary_transport(&self) -> Option<&'a dyn SummaryTransportProvider> {
        self.summary_transport
    }

    pub fn attachment(&self) -> Option<&'a dyn AttachmentProvider> {
        self.attachment
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogError {
    Unreachable(String),
    Backend(String),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(detail) => write!(f, "unreachable: {detail}"),
            Self::Backend(detail) => f.write_str(detail),
        }
    }
}

impl std::error::Error for CatalogError {}

/// Runtime state a session-control provider needs to build a handle.
/// Connection generations are lane-keyed, so the context carries no
/// backend-specific host identity.
pub struct ControlCtx<'a> {
    /// Client locator used by backends with an embedded local terminal.
    pub local_client: &'a str,
    /// Per-lane connection generation/token. Backends without reconnectable
    /// clients can ignore it; absent lanes read as zero.
    pub connection_generations: &'a HashMap<LaneId, u64>,
}

/// Neutral refresh-time context. `client_locator` is an opaque identifier for
/// the embedded client (a tty for tmux); systems without such a client ignore
/// it.
pub struct SnapshotCtx<'a> {
    pub probe_agents: bool,
    pub client_locator: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotMode {
    Foreground,
    Background,
}

/// What a [`System`] tells the shell to draw for one section. Replaces the old
/// hardcoded local/host dividers and the closed `DividerButton` list.
#[derive(Debug, Clone)]
pub struct SectionDef {
    /// Identity of this section's lane.
    pub lane: LaneId,
    /// Divider title (e.g. `"local"`, `"myhost"`). System-defined. Must stand
    /// alone: overlays, tab labels and compact rows all name the lane with it,
    /// away from any divider that would supply context.
    pub title: String,
    /// The lane this one hangs under, when its system mounted it beneath
    /// another. The shell indents such a section's divider and folds it away
    /// with its parent — it never decodes a lane payload to work the
    /// relationship out, so a system that nests lanes has to say so here.
    pub parent: Option<LaneId>,
    /// Shorter label for this section's own divider, for when [`title`] repeats
    /// what the parent's divider already shows one row up (a container reads
    /// `host/name` everywhere else, but under its host it is just `name`).
    /// `None` draws [`title`].
    ///
    /// [`title`]: Self::title
    pub divider_title: Option<String>,
    /// Buttons on the divider, left→right.
    pub buttons: Vec<SectionButton>,
    /// Give this section's header a 1-row top margin (vs. flush).
    pub top_margin: bool,
    /// Whether this lane is backed by Deck's embedded local terminal. Exactly
    /// one built-in lane has this role; other foreground systems must not be
    /// mistaken for it.
    pub primary: bool,
    pub session_capabilities: SessionCapabilities,
    pub lane_capabilities: LaneCapabilities,
}

/// One lane's successful refresh result.
#[derive(Debug, Clone)]
pub struct LaneSnapshot {
    pub sessions: Vec<SessionSnapshot>,
    /// Detected agents, or `None` when not probed this round (the Agents tab
    /// was inactive) or the agent probe failed — distinct from `Some(vec![])`
    /// (probed, none found), which the shell uses to drop stale agents.
    pub agents: Option<Vec<DetectedAgent>>,
}

/// The lock every test that calls `System::configure` holds, wherever in the
/// suite it lives. Not inside the module's own `tests` below: the unit tests
/// under `tests/unit/**` are `#[path]`-included into this same binary and
/// configure systems too, so a lock only that module could name left them
/// racing — the flake this exists to stop, still flaking.
#[cfg(test)]
pub(crate) mod serial {
    /// `TmuxSystem::configure` replaces a *process-wide* container-options
    /// table with the calling instance's view of it, so two tests configuring
    /// their own systems in parallel silently overwrite each other's entries —
    /// which showed up as intermittent failures on whichever engine lost the
    /// race. Every test that configures a system takes this.
    static CONTAINER_TABLE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Hold [`CONTAINER_TABLE`] for the rest of the test, ignoring poisoning
    /// from an unrelated failure so one panic does not cascade.
    pub(crate) fn configure_lock() -> std::sync::MutexGuard<'static, ()> {
        CONTAINER_TABLE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::serial::configure_lock as serial;
    use super::*;

    struct TestSystem;

    impl System for TestSystem {
        fn id(&self) -> &str {
            "test"
        }

        fn lanes(&self) -> Vec<LaneId> {
            vec![
                LaneId::new(self.id(), "primary"),
                LaneId::new(self.id(), "catalog-only"),
            ]
        }

        fn section_for(&self, lane: &LaneId) -> Option<SectionDef> {
            (lane.system() == self.id()).then(|| SectionDef {
                lane: lane.clone(),
                title: lane.lane().into(),
                parent: None,
                divider_title: None,
                buttons: vec![SectionButton {
                    glyph: "!".into(),
                    action: LaneActionId::from("refresh"),
                }],
                top_margin: true,
                primary: false,
                // Deliberately stale declarations: the registry replaces
                // these values from the runtime composition below.
                session_capabilities: SessionCapabilities {
                    activate: true,
                    rename: true,
                    kill: true,
                },
                lane_capabilities: LaneCapabilities::default(),
            })
        }

        fn runtime(&self, lane: &LaneId) -> Option<LaneRuntime<'_>> {
            if lane.system() != self.id() {
                return None;
            }
            let runtime = LaneRuntime::new(lane).with_catalog(self);
            Some(if lane.lane() == "primary" {
                runtime.with_lane_actions(self).with_capabilities(
                    SessionCapabilities::default(),
                    LaneCapabilities {
                        create_session: false,
                        reorder_sessions: false,
                        actions: true,
                        port_forwards: false,
                        forward_endpoint: crate::system::ForwardEndpointKind::Explicit,
                        mounts: false,
                    },
                )
            } else {
                runtime
            })
        }
    }

    impl SessionCatalog for TestSystem {
        fn snapshot(
            &self,
            lane: &LaneId,
            _ctx: &SnapshotCtx<'_>,
        ) -> Result<LaneSnapshot, CatalogError> {
            if lane.system() != self.id() {
                return Err(CatalogError::Backend("lane is not owned by test".into()));
            }
            Ok(LaneSnapshot {
                sessions: vec![crate::model::session::SessionSnapshot {
                    name: "fixture".into(),
                    dir: "/fixture".into(),
                    activity: 0,
                    order: None,
                    is_current: true,
                }],
                agents: Some(vec![]),
            })
        }
    }

    impl LaneActionProvider for TestSystem {
        fn invoke(
            &self,
            _lane: &LaneId,
            _action: &LaneActionId,
            _anchor: LaneActionAnchor,
        ) -> Vec<LaneShellIntent> {
            vec![LaneShellIntent::OpenContextMenu {
                anchor: LaneActionAnchor { x: 1, y: 2 },
            }]
        }
    }

    #[test]
    fn unknown_lane_does_not_fall_back_to_tmux() {
        let registry = builtin_registry();
        let lane = LaneId::new("fake-second-system", "primary");
        assert!(registry.runtime(&lane).is_none());
    }

    #[test]
    fn registered_lane_resolves_its_owner() {
        let registry = builtin_registry();
        let lane = tmux::TmuxSystem::local_lane();
        let runtime = registry.runtime(&lane).expect("tmux runtime");
        assert!(runtime.catalog().is_some());
        assert!(runtime.session_control().is_some());
        assert!(runtime.lane_actions().is_some());
        assert!(runtime.focus_transport().is_some());
        assert!(runtime.summary_transport().is_some());
        assert!(runtime.attachment().is_some());
    }

    #[test]
    fn local_tmux_divider_is_the_menu_every_lane_has() {
        // `configure` replaces the process-wide container-options table
        // wholesale, so it has to take the lock even when the case under test
        // is the *local* divider and the remote list is empty: without it, this
        // wiped the table out from under a concurrent mount test, whose
        // container then read back the default engine instead of its own.
        let _serial = serial();
        let system = tmux::TmuxSystem::default();
        system.configure(&Config::default(), &[]);
        let lane = tmux::TmuxSystem::local_lane();
        let section = system.section_for(&lane).expect("local section");
        // Just `…`, the same button every other lane ends on. `Show hidden`
        // lives in there and is the only way back from hiding a session, so
        // this lane must never be left without it.
        let glyphs: Vec<&str> = section
            .buttons
            .iter()
            .map(|button| button.glyph.as_str())
            .collect();
        assert_eq!(glyphs, vec!["…"]);

        let intents = system.invoke(
            &lane,
            &section.buttons[0].action,
            LaneActionAnchor { x: 4, y: 5 },
        );
        assert_eq!(
            intents,
            vec![LaneShellIntent::OpenContextMenu {
                anchor: LaneActionAnchor { x: 4, y: 5 }
            }]
        );
    }

    #[test]
    fn partial_system_mounts_snapshot_and_actions_without_dummy_control() {
        let test = TestSystem;
        let registry = SystemRegistry::new(vec![&test]);
        let sections = registry.sections();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].title, "primary");
        assert_eq!(
            sections[0].session_capabilities,
            SessionCapabilities::default()
        );
        assert!(sections[0].lane_capabilities.actions);
        assert_eq!(sections[0].buttons.len(), 1);
        assert!(sections[1].buttons.is_empty());

        let runtime = registry
            .snapshot_routes()
            .into_iter()
            .find(|runtime| runtime.lane().lane() == "primary")
            .expect("snapshot route");
        let lane = runtime.lane().clone();
        let snapshot = runtime
            .catalog()
            .expect("catalog port")
            .snapshot(
                &lane,
                &SnapshotCtx {
                    probe_agents: true,
                    client_locator: "fixture-client",
                },
            )
            .expect("snapshot");
        assert_eq!(snapshot.sessions[0].name, "fixture");
        assert!(runtime.session_control().is_none());
        assert!(runtime.lane_config().is_none());
        assert!(runtime.focus_transport().is_none());
        assert!(runtime.summary_transport().is_none());
        assert!(runtime.attachment().is_none());
        assert!(registry
            .config_provider(&SystemId::new(test.id()))
            .is_none());
        assert!(matches!(
            runtime
                .lane_actions()
                .expect("lane action port")
                .invoke(
                    &lane,
                    &LaneActionId::from("refresh"),
                    LaneActionAnchor { x: 1, y: 2 },
                )
                .as_slice(),
            [LaneShellIntent::OpenContextMenu {
                anchor: LaneActionAnchor { x: 1, y: 2 }
            }]
        ));
    }

    #[test]
    fn backend_catalog_failure_remains_distinct_from_unreachable() {
        assert_eq!(
            CatalogError::Backend("malformed response".into()).to_string(),
            "malformed response"
        );
        assert_eq!(
            CatalogError::Unreachable("timeout".into()).to_string(),
            "unreachable: timeout"
        );
    }

    #[test]
    fn configured_containers_become_their_own_lanes_with_titled_sections() {
        let _serial = serial();
        let system = tmux::TmuxSystem::default();
        let remotes = vec![crate::config::RemoteConfig {
            host: "devbox".into(),
            forward_agent: true,
            containers: vec![crate::config::ContainerConfig {
                name: "dev".into(),
                engine: "docker".into(),
                agent_sock: None,
                forwards: vec![],
            }],
            forwards: vec![],
        }];
        system.configure(&Config::default(), &remotes);

        let lanes = system.lanes();
        let container = tmux::TmuxSystem::container_lane("devbox", "dev");
        assert!(lanes.contains(&tmux::TmuxSystem::host_lane("devbox")));
        assert!(lanes.contains(&container));

        // The container divider: readable title, reconnect + menu buttons
        // (no ⇄ badge — container forwards aren't a feature), and the full
        // managed runtime so it attaches/controls like any remote lane.
        let section = system.section_for(&container).expect("container section");
        assert_eq!(section.title, "devbox/dev");
        // Nested under the host it runs on: the shell indents the divider there
        // and folds it away with the host, without decoding the lane id to work
        // out that a container belongs to one.
        assert_eq!(section.parent, Some(tmux::TmuxSystem::host_lane("devbox")));
        assert_eq!(section.divider_title.as_deref(), Some("dev"));
        assert!(
            !section.top_margin,
            "a container is inside its host's block, not a new one"
        );
        let host = system
            .section_for(&tmux::TmuxSystem::host_lane("devbox"))
            .expect("host section");
        assert_eq!(host.parent, None);
        assert!(host.top_margin);
        let cmds: Vec<&str> = section.buttons.iter().map(|b| b.action.as_str()).collect();
        assert_eq!(cmds, ["reconnect", "menu"]);
        assert_eq!(
            AttachmentProvider::role(&system, &container),
            Some(AttachmentRole::Managed)
        );

        assert_eq!(
            tmux::remote_ids(&remotes),
            vec!["devbox".to_string(), "devbox#dev".to_string()]
        );
    }

    /// Mounting records the container in the lane set the caller persists, so
    /// the container is there again next launch. It used to be scoped to one
    /// run because the only place to write it was the config file; the lane set
    /// has its own file now, and "the containers I was working in" is exactly
    /// what that file is for.
    #[test]
    fn a_mounted_container_joins_the_lane_set_the_caller_persists() {
        let _serial = serial();
        let system = tmux::TmuxSystem::default();
        let _config = Config::default();
        let mut remotes = vec![crate::config::RemoteConfig {
            host: "devbox".into(),
            forward_agent: true,
            containers: vec![],
            forwards: vec![],
        }];
        system.configure(&Config::default(), &remotes);
        let host = tmux::TmuxSystem::host_lane("devbox");
        let candidate = format!("podman{}web", '\x1f');

        let mounted =
            LaneMountProvider::mount(&system, &host, &candidate, &mut remotes).expect("mounted");
        assert_eq!(mounted, tmux::TmuxSystem::container_lane("devbox", "web"));
        // Live immediately: the shell onboards the lane on a worker thread as
        // soon as it has the id, without waiting for the commit below.
        assert!(system.lanes().contains(&mounted));
        // And written into the caller's list, which is what reaches the file.
        assert_eq!(remotes[0].containers.len(), 1);
        assert_eq!(remotes[0].containers[0].name, "web");
        assert_eq!(
            remotes[0].containers[0].engine, "podman",
            "the engine discovery found has to survive the restart too"
        );
        // The engine reaches the transport, which is what `<engine> exec` uses.
        assert_eq!(
            crate::remote_tmux::container_opts("devbox#web").engine,
            "podman"
        );

        // Committing the caller's list is idempotent, not a second lane.
        system.configure(&Config::default(), &remotes);
        assert_eq!(
            system
                .lanes()
                .iter()
                .filter(|lane| **lane == mounted)
                .count(),
            1,
            "the mount and the committed entry are one lane, not two"
        );

        // Removing it reports Removed so the shell offboards the lane rather
        // than warning at the user.
        assert_eq!(
            LaneConfigProvider::remove_lane(&system, &mounted, &mut remotes),
            LaneConfigOutcome::Removed
        );
        assert!(remotes[0].containers.is_empty());
        system.configure(&Config::default(), &remotes);
        assert!(!system.lanes().contains(&mounted));
        assert_eq!(
            LaneConfigProvider::remove_lane(&system, &mounted, &mut remotes),
            LaneConfigOutcome::Unsupported
        );
    }

    #[test]
    fn a_containers_divider_counts_its_own_forwards() {
        let _serial = serial();
        let system = tmux::TmuxSystem::default();
        let forward = |port| crate::forwards::ForwardSpec {
            mode: crate::forwards::ForwardMode::Local,
            bind_addr: None,
            listen_port: port,
            target_host: None,
            target_port: Some(80),
        };
        let remotes = vec![crate::config::RemoteConfig {
            host: "devbox".into(),
            forward_agent: true,
            forwards: vec![forward(8080)],
            containers: vec![crate::config::ContainerConfig {
                name: "dev".into(),
                engine: "docker".into(),
                agent_sock: None,
                forwards: vec![forward(9000), forward(9001)],
            }],
        }];
        system.configure(&Config::default(), &remotes);

        let badge = |lane: &LaneId| {
            system
                .section_for(lane)
                .expect("section")
                .buttons
                .first()
                .map(|button| button.glyph.clone())
        };
        // Each divider counts its own rules. The container's live nested inside
        // its host's entry, so a lookup keyed on the host — which is what this
        // was — found none and drew no badge at all.
        assert_eq!(
            badge(&tmux::TmuxSystem::host_lane("devbox")).as_deref(),
            Some("⇄1")
        );
        assert_eq!(
            badge(&tmux::TmuxSystem::container_lane("devbox", "dev")).as_deref(),
            Some("⇄2")
        );
    }

    #[test]
    fn a_mounted_container_goes_when_its_host_does() {
        let _serial = serial();
        let system = tmux::TmuxSystem::default();
        let mut remotes = vec![crate::config::RemoteConfig {
            host: "devbox".into(),
            forward_agent: true,
            containers: vec![],
            forwards: vec![],
        }];
        system.configure(&Config::default(), &remotes);
        let host = tmux::TmuxSystem::host_lane("devbox");
        // A container name of its own: mounting publishes the engine into the
        // process-wide transport table, keyed `host#container`, so two tests
        // sharing a name would race over one entry when run in parallel.
        let mounted = LaneMountProvider::mount(
            &system,
            &host,
            &format!("docker{}api", '\x1f'),
            &mut remotes,
        )
        .unwrap();
        assert!(system.lanes().contains(&mounted));

        // Its lane id names a host Deck can no longer reach, so it cannot outlive
        // the host entry.
        remotes.clear();
        system.configure(&Config::default(), &remotes);
        assert!(!system.lanes().contains(&mounted));
    }

    /// A container on this machine is a lane like any other, and it hangs under
    /// the local section because `host_lane(LOCAL)` *is* the local lane — the
    /// whole reason the sentinel is spelled that way. What it must not have is
    /// anything that only makes sense over ssh.
    #[test]
    fn a_local_container_is_a_lane_under_the_local_section() {
        let _serial = serial();
        let system = tmux::TmuxSystem::default();
        let mut remotes = vec![crate::config::RemoteConfig {
            host: "local".into(),
            forward_agent: true,
            containers: vec![],
            forwards: vec![],
        }];
        let local = tmux::TmuxSystem::local_lane();
        let mounted = LaneMountProvider::mount(
            &system,
            &local,
            &format!("container{}bench", '\x1f'),
            &mut remotes,
        )
        .expect("the local lane mounts containers");
        system.configure(&Config::default(), &remotes);

        let lanes = system.lanes();
        assert!(lanes.contains(&mounted), "{lanes:?}");
        // Exactly one local section: the entry that carries the containers is
        // not a second host lane.
        assert_eq!(
            lanes.iter().filter(|lane| **lane == local).count(),
            1,
            "{lanes:?}"
        );

        let section = system.section_for(&mounted).expect("section");
        assert_eq!(section.parent.as_ref(), Some(&local));
        assert_eq!(section.divider_title.as_deref(), Some("bench"));
        // No forward badge, no reconnect: there is no ssh connection to speak
        // of. What's left is the menu every lane carries.
        let glyphs: Vec<String> = section
            .buttons
            .iter()
            .map(|button| button.glyph.clone())
            .collect();
        assert_eq!(glyphs, vec!["…".to_string()], "{glyphs:?}");
        assert!(!section.lane_capabilities.port_forwards);
        assert!(section.lane_capabilities.create_session);
    }

    #[test]
    fn a_container_is_the_one_lane_that_cannot_mount() {
        let _serial = serial();
        let system = tmux::TmuxSystem::default();
        let remotes = vec![crate::config::RemoteConfig {
            host: "devbox".into(),
            forward_agent: true,
            containers: vec![],
            forwards: vec![],
        }];
        system.configure(&Config::default(), &remotes);

        let mounts = |lane: &LaneId| {
            system
                .runtime(lane)
                .expect("runtime")
                .lane_capabilities
                .mounts
        };
        assert!(mounts(&tmux::TmuxSystem::host_lane("devbox")));
        // The local lane mounts containers on this machine — its engine is one
        // Deck runs directly, with no ssh anywhere in the path.
        assert!(mounts(&tmux::TmuxSystem::local_lane()));
        // A container cannot mount further containers, wherever it runs.
        assert!(!mounts(&tmux::TmuxSystem::container_lane("devbox", "web")));
        assert!(!mounts(&tmux::TmuxSystem::container_lane("local", "dev")));
    }

    #[test]
    fn tmux_lane_config_provider_owns_lane_to_config_translation() {
        let system = tmux::TmuxSystem::default();
        let lane = tmux::TmuxSystem::host_lane("prod");
        let mut remotes = vec![crate::config::RemoteConfig {
            host: "prod".into(),
            containers: vec![],
            forward_agent: true,
            forwards: vec![],
        }];

        assert_eq!(
            LaneConfigProvider::remove_lane(&system, &lane, &mut remotes),
            LaneConfigOutcome::Removed
        );
        assert!(remotes.is_empty());
        assert_eq!(
            LaneConfigProvider::remove_lane(&system, &tmux::TmuxSystem::local_lane(), &mut remotes),
            LaneConfigOutcome::Unsupported
        );

        assert_eq!(
            LaneConfigProvider::add_lane(&system, "next", &mut remotes),
            LaneConfigAddOutcome::Added(tmux::TmuxSystem::host_lane("next"))
        );
        assert_eq!(
            LaneConfigProvider::add_lane(&system, "next", &mut remotes),
            LaneConfigAddOutcome::AlreadyExists
        );
    }

    #[test]
    fn removing_a_container_lane_edits_its_host_entry() {
        let system = tmux::TmuxSystem::default();
        let mut remotes = vec![crate::config::RemoteConfig {
            host: "devbox".into(),
            forward_agent: true,
            containers: vec![
                crate::config::ContainerConfig {
                    name: "dev".into(),
                    engine: "docker".into(),
                    agent_sock: None,
                    forwards: vec![],
                },
                crate::config::ContainerConfig {
                    name: "build".into(),
                    engine: "docker".into(),
                    agent_sock: None,
                    forwards: vec![],
                },
            ],
            forwards: vec![],
        }];

        // A container id names an entry inside its host's list, so removal must
        // reach that list and leave the host (and its other containers) alone.
        assert_eq!(
            LaneConfigProvider::remove_lane(
                &system,
                &tmux::TmuxSystem::container_lane("devbox", "dev"),
                &mut remotes
            ),
            LaneConfigOutcome::Removed
        );
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].containers.len(), 1);
        assert_eq!(remotes[0].containers[0].name, "build");

        // Unknown container, and a container under an unknown host.
        assert_eq!(
            LaneConfigProvider::remove_lane(
                &system,
                &tmux::TmuxSystem::container_lane("devbox", "dev"),
                &mut remotes
            ),
            LaneConfigOutcome::Unsupported
        );
        assert_eq!(
            LaneConfigProvider::remove_lane(
                &system,
                &tmux::TmuxSystem::container_lane("nope", "dev"),
                &mut remotes
            ),
            LaneConfigOutcome::Unsupported
        );

        // Removing the host still takes the whole entry, containers included.
        assert_eq!(
            LaneConfigProvider::remove_lane(
                &system,
                &tmux::TmuxSystem::host_lane("devbox"),
                &mut remotes
            ),
            LaneConfigOutcome::Removed
        );
        assert!(remotes.is_empty());
    }

    #[test]
    fn every_remote_lane_forwards_but_only_a_host_names_its_own_target() {
        let _serial = serial();
        let system = tmux::TmuxSystem::default();
        let mut config = Config::default();
        let remotes = vec![crate::config::RemoteConfig {
            host: "devbox".into(),
            forward_agent: true,
            containers: vec![crate::config::ContainerConfig {
                name: "dev".into(),
                engine: "docker".into(),
                agent_sock: None,
                forwards: vec![],
            }],
            forwards: vec![],
        }];
        system.configure(&Config::default(), &remotes);

        let lane_caps = |lane: &LaneId| system.runtime(lane).expect("runtime").lane_capabilities;
        let caps = |lane: &LaneId| lane_caps(lane).port_forwards;
        // The local lane has no ssh connection anywhere in reach.
        assert!(!caps(&tmux::TmuxSystem::local_lane()));
        assert!(caps(&tmux::TmuxSystem::host_lane("devbox")));
        // A container rides its host's master, which is the connection the `-O`
        // commands address either way, so it forwards too — its rules live in
        // its own config entry.
        let container = tmux::TmuxSystem::container_lane("devbox", "dev");
        assert!(caps(&container));

        // What differs is what a forward points at. A host is a route to
        // wherever the user names; a container is the destination, and where it
        // answers is this system's to resolve on every apply.
        assert_eq!(
            lane_caps(&tmux::TmuxSystem::host_lane("devbox")).forward_endpoint,
            ForwardEndpointKind::Explicit
        );
        assert_eq!(
            lane_caps(&container).forward_endpoint,
            ForwardEndpointKind::Lane
        );

        // Reuse off takes the capability away everywhere: the forward commands
        // are `ssh -O` against the socket it provides.
        config.ssh_connection_reuse = false;
        system.configure(&config, &remotes);
        assert!(!caps(&tmux::TmuxSystem::host_lane("devbox")));
        assert!(!caps(&container));
    }

    #[test]
    fn config_entries_that_cannot_round_trip_through_a_lane_id_are_not_mounted() {
        let _serial = serial();
        let system = tmux::TmuxSystem::default();
        let mut remotes = vec![crate::config::RemoteConfig {
            // `host#` would read back as the host `"devbox#"`, and a `#` in the
            // host would read back as a container lane.
            host: "devbox".into(),
            forward_agent: true,
            containers: vec![
                crate::config::ContainerConfig {
                    name: String::new(),
                    engine: "docker".into(),
                    agent_sock: None,
                    forwards: vec![],
                },
                crate::config::ContainerConfig {
                    name: "dev".into(),
                    engine: "sudo docker".into(),
                    agent_sock: None,
                    forwards: vec![],
                },
                crate::config::ContainerConfig {
                    name: "good".into(),
                    engine: "podman".into(),
                    agent_sock: None,
                    forwards: vec![],
                },
            ],
            forwards: vec![],
        }];
        remotes.push(crate::config::RemoteConfig {
            host: "srv#2".into(),
            forward_agent: true,
            containers: vec![],
            forwards: vec![],
        });
        system.configure(&Config::default(), &remotes);

        let lanes = system.lanes();
        assert!(lanes.contains(&tmux::TmuxSystem::host_lane("devbox")));
        assert!(lanes.contains(&tmux::TmuxSystem::container_lane("devbox", "good")));
        assert_eq!(lanes.len(), 3, "local + devbox + devbox/good: {lanes:?}");
        assert_eq!(
            tmux::remote_ids(&remotes),
            vec!["devbox".to_string(), "devbox#good".to_string()]
        );
    }

    #[test]
    fn tmux_transport_providers_build_backend_specific_targets() {
        let system = tmux::TmuxSystem::default();
        let local = tmux::TmuxSystem::local_lane();
        let remote = tmux::TmuxSystem::host_lane("prod");

        assert_eq!(
            AttachmentProvider::role(&system, &local),
            Some(AttachmentRole::Primary)
        );
        assert_eq!(
            AttachmentProvider::role(&system, &remote),
            Some(AttachmentRole::Managed)
        );

        assert!(matches!(
            FocusTransportProvider::focus_transport(
                &system,
                &local,
                AttachmentEndpoint::Primary {
                    client_locator: "/dev/ttys001",
                },
            ),
            Some(crate::focus::FocusTransport::Local { client_tty })
                if client_tty == "/dev/ttys001"
        ));
        assert!(matches!(
            FocusTransportProvider::focus_transport(
                &system,
                &remote,
                AttachmentEndpoint::Managed { marker_id: 7 },
            ),
            Some(crate::focus::FocusTransport::Remote { host, marker_id })
                if host == "prod" && marker_id == 7
        ));

        let pane =
            SummaryTransportProvider::summary_pane(&system, &remote, "prod:1".into(), "%9".into())
                .expect("summary transport");
        assert_eq!(pane.host.as_deref(), Some("prod"));
        assert_eq!(pane.target, "%9");
    }
}
