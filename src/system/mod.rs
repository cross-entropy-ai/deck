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

use crate::agent::DetectedAgent;
use crate::config::Config;
use crate::effects::Effect;
use crate::forwards::{ForwardHealth, ForwardKey};
use crate::lane::LaneId;
use crate::session::SessionControl;
use crate::tmux::SessionInfo;

/// A mounted backend supplying sessions to the shell. Every method is keyed by
/// [`LaneId`], which already carries the owning system's id — so a system only
/// ever sees lanes it produced in [`sections`](System::sections).
pub trait System {
    /// Stable id for this system (e.g. `"tmux"`). Must match the `system`
    /// half of every [`LaneId`] this system hands out.
    fn id(&self) -> &str;

    /// The lanes this system currently exposes, in display order. Each becomes
    /// one sidebar section (divider header + its rows). The shell concatenates
    /// the sections of all registered systems.
    fn sections(&self, ctx: &SystemCtx) -> Vec<SectionDef>;

    /// Snapshot one lane's sessions + detected agents. Run off the UI thread by
    /// the refresh worker. `None` means the lane was unreachable this round
    /// (distinct from "reachable, empty").
    fn snapshot(&self, lane: &LaneId) -> Option<LaneSnapshot>;

    /// The control-plane handle for one lane (switch/rename/kill/create/…),
    /// run on the executor's per-lane worker thread. `ctx` carries the shell
    /// runtime state a backend may need to construct it (e.g. tmux reads the
    /// local client tty and a remote's reconnect marker id).
    fn control(&self, lane: &LaneId, ctx: &SystemCtx) -> Box<dyn SessionControl + Send>;

    /// Handle a click on a button this system declared on `lane`'s divider,
    /// identified by the button's [`command`](SectionButton::command). Returns
    /// shell effects to enqueue. This is the single seam that lets a system own
    /// button semantics without the reducer growing a per-system arm.
    fn on_button(&self, lane: &LaneId, command: &str) -> Vec<Effect>;
}

/// Read-only shell runtime state a [`System`] may consult when building its
/// sections or control handles. A grab-bag of what the shell can lend a
/// backend; a system ignores the fields it doesn't need. (tmux derives the
/// port-forward badge from `config` + `forward_health`, and builds remote
/// control handles from `local_tty` + `marker_ids`.)
pub struct SystemCtx<'a> {
    pub config: &'a Config,
    pub forward_health: &'a HashMap<ForwardKey, ForwardHealth>,
    /// The local tmux client's tty, stable for the process.
    pub local_tty: &'a str,
    /// Per-host reconnect marker ids (remote connection generation), so a
    /// control handle built now can detect a reconnect. Absent host = 0.
    pub marker_ids: &'a HashMap<String, u64>,
}

/// What a [`System`] tells the shell to draw for one section. Replaces the old
/// hardcoded `@local`/`@host` dividers and the closed `DividerButton` list.
#[derive(Debug, Clone)]
pub struct SectionDef {
    /// Identity of this section's lane.
    pub lane: LaneId,
    /// Divider title (e.g. `"@local"`, `"@myhost"`). System-defined.
    pub title: String,
    /// Theme accent slot index; the shell maps it to a color
    /// (`geometry::host_accent`), keeping color knowledge in the theme.
    pub accent: usize,
    /// Buttons on the divider, left→right.
    pub buttons: Vec<SectionButton>,
    /// Optional status badge (e.g. the `⇄N` port-forward rollup).
    pub badge: Option<Badge>,
    /// Give this section's header a 1-row top margin (vs. flush).
    pub top_margin: bool,
}

/// One divider button. Open-ended: `glyph` is drawn, `command` is an id only
/// the owning system understands and the shell echoes back to
/// [`System::on_button`].
#[derive(Debug, Clone)]
pub struct SectionButton {
    pub glyph: String,
    pub command: String,
}

/// A divider status badge — a label plus a status the shell maps to a theme
/// color. Generalizes the old `ForwardBadge`.
#[derive(Debug, Clone)]
pub struct Badge {
    pub label: String,
    pub status: BadgeStatus,
}

/// Coarse badge status; the shell picks the color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeStatus {
    Ok,
    Warn,
    Err,
    Idle,
}

/// One lane's refresh result.
#[derive(Debug, Clone)]
pub struct LaneSnapshot {
    pub sessions: Vec<SessionInfo>,
    pub agents: Vec<DetectedAgent>,
}
