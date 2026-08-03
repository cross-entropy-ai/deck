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
use crate::effects::Effect;
use crate::geometry::SectionButton;
use crate::lane::LaneId;
use crate::model::session::SessionSnapshot;
use crate::session::SessionControl;

/// Explicit collection of mounted systems. App and background workers receive
/// the same registry reference; model code consumes the resulting
/// [`SectionDef`] values and never performs global backend lookup.
pub struct SystemRegistry<'a> {
    systems: Vec<&'a dyn System>,
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

    /// Resolve the owner of a lane. Unknown ids stay explicit rather than
    /// falling through to a default backend.
    pub fn for_lane(&self, lane: &LaneId) -> Option<&'a dyn System> {
        self.systems
            .iter()
            .copied()
            .find(|system| system.id() == lane.system())
    }

    /// Materialize display definitions in registry/lane order. These values
    /// cross into the model; the backend objects do not.
    pub fn sections(&self) -> Vec<SectionDef> {
        self.systems
            .iter()
            .flat_map(|system| {
                system
                    .lanes()
                    .into_iter()
                    .filter_map(|lane| system.section_for(&lane))
            })
            .collect()
    }

    /// Snapshot routing pairs in registry/lane order. Used by the refresh
    /// worker so adding a System automatically adds its lanes to polling.
    pub fn snapshot_routes(&self) -> Vec<(&'a dyn System, LaneId)> {
        self.systems
            .iter()
            .flat_map(|system| system.lanes().into_iter().map(|lane| (*system, lane)))
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

    /// Snapshot one lane's sessions + detected agents. Run off the UI thread by
    /// the refresh worker. `None` means the lane was unreachable this round
    /// (distinct from a reachable lane with no sessions). `probe_agents` is the
    /// shell's hint that the Agents tab is active — when false a backend should
    /// skip the (possibly expensive) agent detection and leave
    /// [`LaneSnapshot::agents`] `None`.
    fn snapshot(&self, lane: &LaneId, ctx: &SnapshotCtx<'_>) -> Option<LaneSnapshot>;

    /// Whether a lane should be sampled inline with the coalesced refresh
    /// worker or on the guarded parallel background path.
    fn snapshot_mode(&self, _lane: &LaneId) -> SnapshotMode {
        SnapshotMode::Background
    }

    /// The control-plane handle for one lane (switch/rename/kill/create/…),
    /// run on the executor's per-lane worker thread. `ctx` carries the shell
    /// runtime state a backend may need to construct it (e.g. tmux reads the
    /// local client tty and a remote's reconnect marker id).
    fn control(&self, lane: &LaneId, ctx: &ControlCtx) -> Box<dyn SessionControl + Send>;

    /// Handle a click on a button this system declared on `lane`'s divider,
    /// identified by the button's [`command`](SectionButton::command). `(x, y)`
    /// is the button's screen position, for commands that open positioned UI
    /// (e.g. a context menu). Returns shell effects to enqueue. This is the
    /// single seam that lets a system own button semantics without the reducer
    /// growing a per-system arm.
    fn on_button(&self, lane: &LaneId, command: &str, x: u16, y: u16) -> Vec<Effect>;
}

/// Runtime state a [`System`] needs to build a [`control`](System::control)
/// handle. Connection generations are lane-keyed, so the context carries no
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
    /// mistaken for it merely because they have no runtime connection key.
    pub primary: bool,
    /// Optional runtime connection key. The shell treats it as opaque; the
    /// built-in tmux system uses the SSH host for reconnect/PTY workflows.
    pub runtime_key: Option<String>,
}

/// One lane's refresh result. Returned inside `Option` — `None` from
/// [`snapshot`](System::snapshot) means the lane was unreachable.
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

    struct TestControl;

    impl SessionControl for TestControl {
        fn switch_to(&self, _name: &str) -> crate::session::SessionControlResult {
            Ok(())
        }

        fn rename(&self, _old: &str, _new: &str) -> crate::session::SessionControlResult {
            Ok(())
        }

        fn kill(&self, _name: &str) -> crate::session::SessionControlResult {
            Ok(())
        }

        fn create(&self, _name: &str, _dir: &str) -> crate::session::SessionControlResult {
            Ok(())
        }

        fn persist_order(&self, _order: &[String]) -> crate::session::SessionControlResult {
            Ok(())
        }

        fn list_dir(
            &self,
            _path: &str,
        ) -> crate::session::SessionControlResult<crate::session::DirListing> {
            Ok(crate::session::DirListing { entries: vec![] })
        }
    }

    struct TestSystem;

    impl System for TestSystem {
        fn id(&self) -> &str {
            "test"
        }

        fn lanes(&self) -> Vec<LaneId> {
            vec![LaneId::new(self.id(), "primary")]
        }

        fn section_for(&self, lane: &LaneId) -> Option<SectionDef> {
            (lane.system() == self.id()).then(|| SectionDef {
                lane: lane.clone(),
                title: "test backend".into(),
                buttons: vec![],
                top_margin: true,
                primary: false,
                runtime_key: None,
            })
        }

        fn snapshot(&self, lane: &LaneId, _ctx: &SnapshotCtx<'_>) -> Option<LaneSnapshot> {
            (lane.system() == self.id()).then(|| LaneSnapshot {
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

        fn control(&self, _lane: &LaneId, _ctx: &ControlCtx) -> Box<dyn SessionControl + Send> {
            Box::new(TestControl)
        }

        fn on_button(&self, _lane: &LaneId, _command: &str, _x: u16, _y: u16) -> Vec<Effect> {
            vec![]
        }
    }

    #[test]
    fn unknown_lane_does_not_fall_back_to_tmux() {
        let registry = builtin_registry();
        let lane = LaneId::new("fake-second-system", "primary");
        assert!(registry.for_lane(&lane).is_none());
    }

    #[test]
    fn registered_lane_resolves_its_owner() {
        let registry = builtin_registry();
        let lane = tmux::TmuxSystem::local_lane();
        assert_eq!(registry.for_lane(&lane).map(System::id), Some(tmux::TMUX));
    }

    #[test]
    fn second_system_mounts_sections_snapshots_and_control_without_shell_changes() {
        let test = TestSystem;
        let registry = SystemRegistry::new(vec![&test]);
        let sections = registry.sections();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, "test backend");

        let (system, lane) = registry.snapshot_routes().pop().expect("snapshot route");
        let snapshot = system
            .snapshot(
                &lane,
                &SnapshotCtx {
                    probe_agents: true,
                    client_locator: "fixture-client",
                },
            )
            .expect("snapshot");
        assert_eq!(snapshot.sessions[0].name, "fixture");

        let generations = HashMap::new();
        let control = registry.for_lane(&lane).expect("lane owner").control(
            &lane,
            &ControlCtx {
                local_client: "fixture-client",
                connection_generations: &generations,
            },
        );
        assert!(control.switch_to("fixture").is_ok());
    }
}
