//! Off-thread discovery and activation for [`crate::system::LaneMountProvider`].
//!
//! Both port methods are documented as blocking: for tmux they are an ssh hop to
//! run `docker ps` or `docker start`, which can take seconds on a cold
//! connection. So the shell never calls them inline — it kicks a one-shot worker
//! thread per request and drains the answer from the run loop, the same shape
//! `ssh::remote_spawn` uses for attach PTYs.
//!
//! Requests carry a `generation`. The picker can be closed and reopened, or
//! pointed at another lane, while a probe is still in flight; a late answer for a
//! superseded request would otherwise repopulate a list the user has moved on
//! from. The generation rides through to the reducer, which owns the picker's
//! current one and drops any answer that no longer matches.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crate::lane::LaneId;
use crate::system::{LaneMountProvider, MountCandidate};

/// One answer from a worker thread.
pub(in crate::app) enum MountEvent {
    Discovered {
        lane: LaneId,
        generation: u64,
        result: Result<Vec<MountCandidate>, String>,
    },
    /// An activation finished. `Ok` means the candidate is now mountable.
    Activated {
        lane: LaneId,
        generation: u64,
        candidate: String,
        result: Result<(), String>,
    },
}

/// Owns the receiving end. Senders live in short-lived worker threads, each of
/// which delivers exactly one event and exits.
pub(in crate::app) struct MountWorker {
    tx: Sender<MountEvent>,
    rx: Receiver<MountEvent>,
}

impl MountWorker {
    pub(in crate::app) fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx }
    }

    /// Ask `provider` what `lane` could mount, off the UI thread.
    pub(in crate::app) fn discover(
        &self,
        lane: LaneId,
        generation: u64,
        provider: &'static dyn LaneMountProvider,
    ) {
        let tx = self.tx.clone();
        // A failed spawn is reported through the same channel, so the picker
        // leaves its loading state either way rather than hanging forever.
        let spawned = thread::Builder::new()
            .name("deck-mount-discover".into())
            .spawn(move || {
                let result = provider.discover(&lane);
                let _ = tx.send(MountEvent::Discovered {
                    lane,
                    generation,
                    result,
                });
            });
        if let Err(error) = spawned {
            let _ = self.tx.send(MountEvent::Discovered {
                lane: LaneId::new("", ""),
                generation,
                result: Err(error.to_string()),
            });
        }
    }

    /// Bring `candidate` into a mountable state, off the UI thread.
    pub(in crate::app) fn activate(
        &self,
        lane: LaneId,
        generation: u64,
        candidate: String,
        provider: &'static dyn LaneMountProvider,
    ) {
        let tx = self.tx.clone();
        let spawned = thread::Builder::new()
            .name("deck-mount-activate".into())
            .spawn(move || {
                let result = provider.activate(&lane, &candidate);
                let _ = tx.send(MountEvent::Activated {
                    lane,
                    generation,
                    candidate,
                    result,
                });
            });
        if let Err(error) = spawned {
            let _ = self.tx.send(MountEvent::Activated {
                lane: LaneId::new("", ""),
                generation,
                candidate: String::new(),
                result: Err(error.to_string()),
            });
        }
    }

    pub(in crate::app) fn try_recv(&self) -> Option<MountEvent> {
        self.rx.try_recv().ok()
    }
}
