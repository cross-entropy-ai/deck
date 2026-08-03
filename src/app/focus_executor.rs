//! Execution boundary for potentially blocking pane focus/probe work.
//!
//! Dispatch decides *what* focus operation is needed; this service owns thread
//! creation, channels, and result transport so UI policy never spawns ad-hoc
//! threads or silently drops spawn failures.

use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crate::focus::FocusTransport;
use crate::geometry::AgentTarget;

pub(crate) struct FocusOutcome {
    pub target: AgentTarget,
    pub result: crate::tmux::PaneFocus,
    pub seq: u64,
    pub marker_id: u64,
}

pub(crate) struct ActivePaneOutcome {
    pub host: Option<String>,
    pub pane_id: Option<String>,
    pub seq: u64,
    pub marker_id: u64,
}

pub(super) struct FocusExecutor {
    focus_tx: Sender<FocusOutcome>,
    focus_rx: Receiver<FocusOutcome>,
    active_pane_tx: Sender<ActivePaneOutcome>,
    active_pane_rx: Receiver<ActivePaneOutcome>,
}

impl FocusExecutor {
    pub fn new() -> Self {
        let (focus_tx, focus_rx) = mpsc::channel();
        let (active_pane_tx, active_pane_rx) = mpsc::channel();
        Self {
            focus_tx,
            focus_rx,
            active_pane_tx,
            active_pane_rx,
        }
    }

    pub fn focus(
        &self,
        transport: FocusTransport,
        target: AgentTarget,
        seq: u64,
        marker_id: u64,
    ) -> io::Result<()> {
        let tx = self.focus_tx.clone();
        thread::Builder::new()
            .name("deck-focus".into())
            .spawn(move || {
                let result = crate::focus::run_focus(&transport, &target.session, &target.pane_id);
                let _ = tx.send(FocusOutcome {
                    target,
                    result,
                    seq,
                    marker_id,
                });
            })
            .map(drop)
    }

    pub fn probe_active_pane(
        &self,
        transport: FocusTransport,
        host: Option<String>,
        seq: u64,
        marker_id: u64,
    ) -> io::Result<()> {
        let tx = self.active_pane_tx.clone();
        thread::Builder::new()
            .name("deck-active-pane".into())
            .spawn(move || {
                let pane_id = crate::focus::active_pane(&transport);
                let _ = tx.send(ActivePaneOutcome {
                    host,
                    pane_id,
                    seq,
                    marker_id,
                });
            })
            .map(drop)
    }

    pub fn try_recv_focus(&self) -> Option<FocusOutcome> {
        self.focus_rx.try_recv().ok()
    }

    pub fn try_recv_active_pane(&self) -> Option<ActivePaneOutcome> {
        self.active_pane_rx.try_recv().ok()
    }
}
