//! The Buddy server's half of the UI thread: who is allowed to drive this Mac,
//! and the bookkeeping that keeps the question answerable.
//!
//! The server itself never asks the UI thread about input — synthesis happens
//! on the connection thread, because mouse movement arrives at gesture rate and
//! a 16 ms poll loop would put jitter on the one thing that has to feel direct.
//! What arrives here is one question per connection, and the answer decides
//! whether that connection is allowed to act at all.
//!
//! The rest of this module is the other half of that: the two messages that
//! *are* about deck rather than about the frontmost app. [`App::buddy_state`]
//! projects the sidebar into the wire DTO, and [`App::apply_buddy_select`]
//! resolves what a client named back onto a live row and dispatches the same
//! action a click on it would.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use crate::geometry::AgentTarget;
use crate::infra::buddy::protocol::{
    AgentInfo, AgentKindName, AgentRef, AgentStatusName, HostInfo, HostStatus, Select, SessionInfo,
    SessionRef, State, Tab,
};
use crate::infra::buddy::{self, BuddyEvent, BuddyServer};
use crate::lane::LaneId;
use crate::state::{AppState, BuddyStatus, SessionEntryKind, SidebarTab};

use super::action::Action;
use super::App;

/// How long a denial is remembered. Long enough that a client cycling through
/// its reconnect backoff cannot put the prompt back up over and over; short
/// enough that a misplaced `n` is not a lockout with no way back but restarting
/// deck.
const DENY_FOR: Duration = Duration::from_secs(60);

/// What to do about a connection, before any prompt is involved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Already known; let it in without bothering the user.
    Allow,
    /// Denied recently; drop it without bothering the user.
    Deny,
    /// Put the question to the user.
    Ask,
}

/// Decide what happens to a connection from `peer`.
///
/// Split out from the worker, and free of any platform or IO type, so the
/// policy is tested on every CI rather than only where the server can run.
pub fn decide(
    peer: IpAddr,
    approved: &[IpAddr],
    denied: &HashMap<IpAddr, Instant>,
    now: Instant,
) -> Verdict {
    if approved.contains(&peer) {
        return Verdict::Allow;
    }
    match denied.get(&peer) {
        Some(at) if now.duration_since(*at) < DENY_FOR => Verdict::Deny,
        _ => Verdict::Ask,
    }
}

/// Owns the running server, the answers given so far, and the queue of
/// questions still to put to the user.
pub(in crate::app) struct BuddyWorker {
    tx: Sender<BuddyEvent>,
    rx: Receiver<BuddyEvent>,
    server: Option<BuddyServer>,
    /// What the running server was started with, so a save that changed
    /// something unrelated doesn't tear down a live iPad session.
    running: Option<(u16, String)>,
    approved: Vec<IpAddr>,
    denied: HashMap<IpAddr, Instant>,
    /// The question on screen, and how to answer it.
    asking: Option<(IpAddr, Sender<bool>)>,
    /// Questions waiting for the screen to be free. A connection is content to
    /// wait: from its own point of view it is connected, since the pong it
    /// gates on is answered throughout.
    queue: VecDeque<(IpAddr, Sender<bool>)>,
}

impl BuddyWorker {
    pub(in crate::app) fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            server: None,
            running: None,
            approved: Vec::new(),
            denied: HashMap::new(),
            asking: None,
            queue: VecDeque::new(),
        }
    }

    pub(in crate::app) fn try_recv(&self) -> Option<BuddyEvent> {
        self.rx.try_recv().ok()
    }

    /// Whether a question is on screen or waiting to be. While this holds, the
    /// server freezes *every* connection — see [`BuddyWorker::regate`].
    pub(in crate::app) fn is_asking(&self) -> bool {
        self.asking.is_some() || !self.queue.is_empty()
    }

    /// Bring the server in line with the current settings, and report what to
    /// show. Returns `None` when nothing changed, so a save triggered by an
    /// unrelated setting doesn't drop a live connection.
    pub(in crate::app) fn reconfigure(
        &mut self,
        enabled: bool,
        port: u16,
        name: &str,
    ) -> Option<BuddyStatus> {
        let wanted = enabled.then(|| (port, name.to_string()));
        if wanted == self.running {
            return None;
        }
        self.stop();
        let Some((port, name)) = wanted else {
            return Some(BuddyStatus::Off);
        };
        match buddy::start(port, &name, self.tx.clone()) {
            Ok(server) => {
                let status = BuddyStatus::Listening {
                    port: server.port,
                    name: server.name.clone(),
                    clients: 0,
                };
                self.running = Some((server.port, server.name.clone()));
                self.server = Some(server);
                Some(status)
            }
            Err(err) => Some(BuddyStatus::Failed(err)),
        }
    }

    /// Stop listening and forget the pending questions — but not the answers,
    /// so a device approved before a restart is not asked about again.
    pub(in crate::app) fn stop(&mut self) {
        if let Some(mut server) = self.server.take() {
            server.stop();
        }
        self.running = None;
        self.asking = None;
        self.queue.clear();
    }

    /// Answer a new connection from what the user has already said, or queue
    /// it. Only [`Self::next_question`] raises a prompt, so one that has to
    /// wait for the screen is still raised once it frees.
    pub(in crate::app) fn connected(&mut self, peer: IpAddr, reply: Sender<bool>) {
        match decide(peer, &self.approved, &self.denied, Instant::now()) {
            Verdict::Allow => {
                let _ = reply.send(true);
            }
            // Dropping the sender is itself the denial; saying so is clearer.
            Verdict::Deny => {
                let _ = reply.send(false);
            }
            Verdict::Ask => self.queue.push_back((peer, reply)),
        }
    }

    /// Put the next queued question on screen if there is room for it.
    ///
    /// Nothing is asked while another overlay is up: the modal slot holds one
    /// thing at a time and `open` replaces it, so a connection arriving
    /// mid-rename would silently destroy what the user was typing.
    pub(in crate::app) fn next_question(&mut self, screen_busy: bool) -> Option<IpAddr> {
        if self.asking.is_some() || screen_busy {
            return None;
        }
        let (peer, reply) = self.queue.pop_front()?;
        self.asking = Some((peer, reply));
        Some(peer)
    }

    /// Answer the question on screen. Returns a peer the caller has to persist
    /// — an allow for a device not already in the saved list.
    pub(in crate::app) fn answer(&mut self, allow: bool) -> Option<IpAddr> {
        let (peer, reply) = self.asking.take()?;
        let mut fresh = None;
        if allow {
            if !self.approved.contains(&peer) {
                self.approved.push(peer);
                fresh = Some(peer);
            }
            self.denied.remove(&peer);
        } else {
            self.denied.insert(peer, Instant::now());
        }
        let _ = reply.send(allow);
        fresh
    }

    /// Adopt the addresses approved in earlier runs.
    ///
    /// Replaces rather than merges, so deleting an entry from the config file
    /// revokes that device on the next reload. Anything approved in this
    /// session is already written back to the config before this runs.
    /// Unparseable entries are dropped; config validation reports them.
    pub(in crate::app) fn restore_approved(&mut self, saved: &[String]) {
        self.approved = saved.iter().filter_map(|s| s.parse().ok()).collect();
    }

    /// The peer the prompt on screen is about.
    #[cfg(test)]
    pub(in crate::app) fn asking_peer(&self) -> Option<IpAddr> {
        self.asking.as_ref().map(|(peer, _)| *peer)
    }

    /// Forget a peer that went away. Returns whether the question on screen
    /// was the one it had gone from — there is nothing left to answer, so the
    /// caller takes the prompt down rather than leaving the user deciding about
    /// a device that is no longer there.
    pub(in crate::app) fn disconnected(&mut self, peer: IpAddr) -> bool {
        let was_asking = self.asking.as_ref().is_some_and(|(p, _)| *p == peer);
        if was_asking {
            self.asking = None;
        }
        self.queue.retain(|(p, _)| *p != peer);
        was_asking
    }

    /// Freeze or unfreeze every connection to match whether a prompt is up.
    ///
    /// An approved device can type; the prompt is answered by typing `y`. So
    /// while any question is outstanding, nothing is allowed to act — otherwise
    /// one approved device could let a stranger in on the user's behalf.
    pub(in crate::app) fn regate(&self) {
        if let Some(server) = &self.server {
            server.set_gated(self.is_asking());
        }
    }

    /// How many devices are connected right now, for the settings row.
    pub(in crate::app) fn clients(&self) -> usize {
        self.server
            .as_ref()
            .map_or(0, BuddyServer::connection_count)
    }
}

impl App {
    /// What the sidebar is showing, as the `state` reply.
    pub(super) fn buddy_state(&self) -> State {
        snapshot(&self.state)
    }

    /// Move the selection a client named, by dispatching exactly the action a
    /// click on that row would.
    ///
    /// Everything is resolved against the live lists rather than trusted as an
    /// index: the client is describing a `state` reply that may be several
    /// refreshes old, and a row that has since gone is a no-op, not a switch to
    /// whatever slid into its place.
    pub(super) fn apply_buddy_select(&mut self, select: Select) {
        match select {
            Select::Tab(tab) => {
                self.dispatch(Action::SelectTab(match tab {
                    Tab::Projects => SidebarTab::Projects,
                    Tab::Agents => SidebarTab::Agents,
                }));
            }
            Select::Session(want) => {
                let Some(idx) = session_index(&self.state, &want) else {
                    return;
                };
                // `FocusIndex` and `SwitchProject` both act on whichever tab is
                // active, so asking for a session has to mean showing sessions
                // — otherwise this would move the *agent* cursor instead.
                self.dispatch(Action::SelectTab(SidebarTab::Projects));
                self.dispatch(Action::SidebarClickSession(idx));
            }
            Select::Agent(want) => {
                let Some(target) = agent_target(&self.state, &want) else {
                    return;
                };
                // No tab switch: switching to an agent's pane works from either
                // tab (a section footer offers it on Projects), and the active-
                // pane probe steers the Agents cursor onto it either way.
                self.dispatch(Action::SwitchToAgentPane(target));
            }
        }
    }
}

/// What the sidebar is showing, as the `state` reply.
///
/// Reads the same fields the renderer does, in the same order, so the client's
/// list and the sidebar agree row for row. Only selectable rows are listed: a
/// lane's synthetic status row ("connecting…", "unreachable") is not a session,
/// so it becomes that lane's [`HostStatus`] instead.
fn snapshot(state: &AppState) -> State {
    State {
        // The tab actually being rendered, not the stored preference. A
        // narrow terminal has no tab bar, so `sidebar_tab` can say Agents
        // while the sidebar is showing sessions — reporting the preference
        // would have the client draw a list deck isn't showing.
        tab: if state.agents_tab_active() {
            Tab::Agents
        } else {
            Tab::Projects
        },
        hosts: state
            .system_sections
            .iter()
            .map(|section| HostInfo {
                lane: section.lane.as_str().to_string(),
                title: section.title.clone(),
                parent: section.parent.as_ref().map(|p| p.as_str().to_string()),
                status: host_status(state, &section.lane),
            })
            .collect(),
        sessions: state
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.is_attachable())
            .map(|(idx, entry)| SessionInfo {
                lane: entry.lane.as_str().to_string(),
                name: entry.name.clone(),
                dir: entry.dir.clone(),
                selected: idx == state.focused,
            })
            .collect(),
        agents: state
            .agent_entries
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| {
                let agent = entry.agent()?;
                Some(AgentInfo {
                    lane: entry.lane.as_str().to_string(),
                    kind: match agent.kind {
                        crate::agent::AgentKind::Claude => AgentKindName::Claude,
                        crate::agent::AgentKind::Codex => AgentKindName::Codex,
                    },
                    session: agent.session.clone(),
                    window: agent.window.clone(),
                    pane: agent.pane_id.clone(),
                    status: match agent.status {
                        crate::agent::AgentStatus::Working => AgentStatusName::Working,
                        crate::agent::AgentStatus::Idle => AgentStatusName::Idle,
                        crate::agent::AgentStatus::Waiting => AgentStatusName::Waiting,
                        crate::agent::AgentStatus::Unknown => AgentStatusName::Unknown,
                    },
                    selected: idx == state.agent_focused,
                })
            })
            .collect(),
    }
}

/// Why `lane` has no sessions listed, when it has none. Its synthetic status
/// row carries the answer; a lane with real rows is simply `Ok`.
fn host_status(state: &AppState, lane: &LaneId) -> HostStatus {
    state
        .entries
        .iter()
        .find(|entry| entry.lane == *lane)
        .map_or(HostStatus::Ok, |entry| match entry.kind {
            SessionEntryKind::Live { .. } => HostStatus::Ok,
            SessionEntryKind::Connecting => HostStatus::Connecting,
            SessionEntryKind::Unreachable => HostStatus::Unreachable,
            SessionEntryKind::NoSessions => HostStatus::NoSessions,
        })
}

/// The flat sidebar index of the session a client named, or `None` if it is no
/// longer listed. Free functions rather than `App` methods so the resolution
/// rules are testable against a bare `AppState`.
fn session_index(state: &AppState, want: &SessionRef) -> Option<usize> {
    state.entries.iter().position(|entry| {
        entry.is_attachable() && entry.lane.as_str() == want.lane && entry.name == want.name
    })
}

/// The switch target for the agent a client named, or `None` if it is gone.
/// Keyed on the pane id alone within its lane, which is the only handle that
/// survives a window being renamed or a pane moving between them.
fn agent_target(state: &AppState, want: &AgentRef) -> Option<AgentTarget> {
    state.agent_entries.iter().find_map(|entry| {
        let agent = entry.agent()?;
        (entry.lane.as_str() == want.lane && agent.pane_id == want.pane).then(|| AgentTarget {
            lane: entry.lane.clone(),
            session: agent.session.clone(),
            pane_id: agent.pane_id.clone(),
        })
    })
}

#[cfg(test)]
#[path = "../../tests/unit/app/buddy.rs"]
mod tests;
