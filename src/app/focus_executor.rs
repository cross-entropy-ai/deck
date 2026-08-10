//! Execution boundary for the read-only active-pane probe.
//!
//! Mutating pane focus runs in `SessionExecutor`'s per-lane FIFO. This service
//! only queries the currently active pane; App single-flights those probes.

use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crate::focus::{ActiveTarget, FocusTransport};
pub(crate) struct ActivePaneOutcome {
    pub lane: crate::lane::LaneId,
    pub target: Result<Option<ActiveTarget>, ActivePaneProbeError>,
    pub seq: u64,
    pub marker_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActivePaneProbeError {
    Panicked,
}

impl std::fmt::Display for ActivePaneProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Panicked => f.write_str("active-pane backend panicked"),
        }
    }
}

pub(super) struct ActivePaneProbeExecutor {
    active_pane_tx: Sender<ActivePaneOutcome>,
    active_pane_rx: Receiver<ActivePaneOutcome>,
}

impl ActivePaneProbeExecutor {
    pub fn new() -> Self {
        let (active_pane_tx, active_pane_rx) = mpsc::channel();
        Self {
            active_pane_tx,
            active_pane_rx,
        }
    }

    pub fn probe_active_pane(
        &self,
        transport: FocusTransport,
        lane: crate::lane::LaneId,
        seq: u64,
        marker_id: u64,
    ) -> io::Result<()> {
        let tx = self.active_pane_tx.clone();
        thread::Builder::new()
            .name("deck-active-pane".into())
            .spawn(move || {
                let target = run_probe(|| crate::focus::active_target(&transport));
                let _ = tx.send(ActivePaneOutcome {
                    lane,
                    target,
                    seq,
                    marker_id,
                });
            })
            .map(drop)
    }

    pub fn try_recv_active_pane(&self) -> Option<ActivePaneOutcome> {
        self.active_pane_rx.try_recv().ok()
    }
}

fn run_probe<T>(probe: impl FnOnce() -> Option<T>) -> Result<Option<T>, ActivePaneProbeError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(probe))
        .map_err(|_| ActivePaneProbeError::Panicked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_panic_is_a_typed_failure() {
        assert_eq!(
            run_probe::<ActiveTarget>(|| panic!("injected probe panic")),
            Err(ActivePaneProbeError::Panicked)
        );
    }
}
