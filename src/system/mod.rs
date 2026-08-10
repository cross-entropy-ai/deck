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
    pub fn configure(&self, config: &Config) {
        for system in &self.systems {
            system.configure(config);
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
    /// config; implementations retain only the subset they understand.
    fn configure(&self, _config: &Config) {}

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

/// Backend-owned configuration mutations addressed by lane identity.
/// The shell supplies the persisted configuration as a whole and applies the
/// typed outcome; only the owning backend interprets its lane representation.
pub trait LaneConfigProvider: Send + Sync {
    fn add_lane(&self, candidate: &str, config: &mut Config) -> LaneConfigAddOutcome;
    fn remove_lane(&self, lane: &LaneId, config: &mut Config) -> LaneConfigOutcome;
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
    OpenNewSession,
    OpenContextMenu { anchor: LaneActionAnchor },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionCapabilities {
    pub activate: bool,
    pub rename: bool,
    pub kill: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LaneCapabilities {
    pub create_session: bool,
    pub reorder_sessions: bool,
    pub actions: bool,
}

#[derive(Clone)]
pub struct LaneRuntime<'a> {
    lane: LaneId,
    catalog: Option<&'a dyn SessionCatalog>,
    session_control: Option<&'a dyn SessionControlProvider>,
    lane_actions: Option<&'a dyn LaneActionProvider>,
    lane_config: Option<&'a dyn LaneConfigProvider>,
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
    /// Divider title (e.g. `"local"`, `"myhost"`). System-defined.
    pub title: String,
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

#[cfg(test)]
mod tests {
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
    fn local_tmux_divider_is_a_single_direct_new_session_action() {
        let system = tmux::TmuxSystem::default();
        system.configure(&Config::default());
        let lane = tmux::TmuxSystem::local_lane();
        let section = system.section_for(&lane).expect("local section");
        assert_eq!(section.buttons.len(), 1);
        assert_eq!(section.buttons[0].glyph, "+");

        let intents = system.invoke(
            &lane,
            &section.buttons[0].action,
            LaneActionAnchor { x: 4, y: 5 },
        );
        assert_eq!(intents, vec![LaneShellIntent::OpenNewSession]);
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
    fn tmux_lane_config_provider_owns_lane_to_config_translation() {
        let system = tmux::TmuxSystem::default();
        let lane = tmux::TmuxSystem::host_lane("prod");
        let mut config = Config::default();
        config.remotes.push(crate::config::RemoteConfig {
            host: "prod".into(),
            forwards: vec![],
        });

        assert_eq!(
            LaneConfigProvider::remove_lane(&system, &lane, &mut config),
            LaneConfigOutcome::Removed
        );
        assert!(config.remotes.is_empty());
        assert_eq!(
            LaneConfigProvider::remove_lane(&system, &tmux::TmuxSystem::local_lane(), &mut config,),
            LaneConfigOutcome::Unsupported
        );

        assert_eq!(
            LaneConfigProvider::add_lane(&system, "next", &mut config),
            LaneConfigAddOutcome::Added(tmux::TmuxSystem::host_lane("next"))
        );
        assert_eq!(
            LaneConfigProvider::add_lane(&system, "next", &mut config),
            LaneConfigAddOutcome::AlreadyExists
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
