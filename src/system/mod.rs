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

/// The mounted systems, in registration order. Adding a backend = implement
/// [`System`], construct it here. (Today: just tmux.) Statics so any layer —
/// the model's layout builder, the infra refresh worker, the app's dispatch —
/// can resolve a lane's owner without threading a registry through them.
static TMUX_SYSTEM: tmux::TmuxSystem = tmux::TmuxSystem;
static SYSTEMS: &[&dyn System] = &[&TMUX_SYSTEM];

/// The [`System`] that owns `lane`, resolved by its `system` id. Falls back to
/// the first registered system for an unknown id (there is only one today).
pub fn for_lane(lane: &LaneId) -> &'static dyn System {
    SYSTEMS
        .iter()
        .copied()
        .find(|s| s.id() == lane.system())
        .unwrap_or(SYSTEMS[0])
}

use crate::agent::DetectedAgent;
use crate::config::RemoteConfig;
use crate::effects::Effect;
use crate::geometry::SectionButton;
use crate::lane::LaneId;
use crate::session::SessionControl;
use crate::tmux::SessionInfo;

/// A mounted backend supplying sessions to the shell. Every method is keyed by
/// [`LaneId`], which already carries the owning system's id — so a system only
/// ever sees lanes it produced in [`section_for`](System::section_for).
pub trait System: Send + Sync {
    /// Stable id for this system (e.g. `"tmux"`). Must match the `system`
    /// half of every [`LaneId`] this system hands out.
    fn id(&self) -> &str;

    /// The [`SectionDef`] for a single lane: the divider's title and buttons.
    /// The shell enumerates the lanes to lay out from its session list (so
    /// every session row keeps a section) and calls this to style each one.
    fn section_for(&self, lane: &LaneId, ctx: &SectionCtx) -> SectionDef;

    /// Snapshot one lane's sessions + detected agents. Run off the UI thread by
    /// the refresh worker. `None` means the lane was unreachable this round
    /// (distinct from a reachable lane with no sessions). `probe_agents` is the
    /// shell's hint that the Agents tab is active — when false a backend should
    /// skip the (possibly expensive) agent detection and leave
    /// [`LaneSnapshot::agents`] `None`.
    fn snapshot(&self, lane: &LaneId, probe_agents: bool) -> Option<LaneSnapshot>;

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

/// Read-only state a [`System`] consults to build its [`section_for`](System::section_for).
/// (tmux derives lanes — and each `⇄N` forward count — from `remotes`.)
pub struct SectionCtx<'a> {
    pub remotes: &'a [RemoteConfig],
}

/// Runtime state a [`System`] needs to build a [`control`](System::control)
/// handle. (tmux builds the local handle from `local_tty`, a remote one from
/// the host's `marker_ids` entry.)
pub struct ControlCtx<'a> {
    /// The local tmux client's tty, stable for the process.
    pub local_tty: &'a str,
    /// Per-host reconnect marker ids (remote connection generation), so a
    /// control handle built now can detect a reconnect. Absent host = 0.
    pub marker_ids: &'a HashMap<String, u64>,
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
}

/// One lane's refresh result. Returned inside `Option` — `None` from
/// [`snapshot`](System::snapshot) means the lane was unreachable.
#[derive(Debug, Clone)]
pub struct LaneSnapshot {
    pub sessions: Vec<SessionInfo>,
    /// Detected agents, or `None` when not probed this round (the Agents tab
    /// was inactive) or the agent probe failed — distinct from `Some(vec![])`
    /// (probed, none found), which the shell uses to drop stale agents.
    pub agents: Option<Vec<DetectedAgent>>,
}
