use std::collections::BTreeSet;
use std::sync::{Mutex, PoisonError};

use buizz_core::run::{RunId, RunOrigin};
use buizz_supervisor::Supervisor;
use tokio::sync::{Notify, OwnedSemaphorePermit};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Place {
    origin: RunOrigin,
    expected_millis: u64,
    run_id: RunId,
}

impl Place {
    pub(crate) fn new(origin: RunOrigin, expected_seconds: f64, run_id: RunId) -> Self {
        let expected_millis = match origin {
            RunOrigin::Request => 0,
            RunOrigin::Screening => (expected_seconds * 1000.0).max(0.0) as u64,
        };
        Self {
            origin,
            expected_millis,
            run_id,
        }
    }
}

#[derive(Default)]
pub(crate) struct BacktestQueue {
    waiting: Mutex<BTreeSet<Place>>,
    turn: Notify,
}

struct Waiting<'a> {
    queue: &'a BacktestQueue,
    place: Place,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.queue.waiting().remove(&self.place);
        self.queue.turn.notify_waiters();
    }
}

impl BacktestQueue {
    fn waiting(&self) -> std::sync::MutexGuard<'_, BTreeSet<Place>> {
        self.waiting.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) async fn slot(&self, place: Place, supervisor: &Supervisor) -> OwnedSemaphorePermit {
        self.waiting().insert(place);
        self.turn.notify_waiters();
        let _waiting = Waiting { queue: self, place };
        loop {
            let turn = self.turn.notified();
            tokio::pin!(turn);
            turn.as_mut().enable();
            if self.waiting().first() == Some(&place) {
                tokio::select! {
                    slot = supervisor.backtest_slot() => return slot,
                    () = &mut turn => {}
                }
            } else {
                turn.await;
            }
        }
    }
}
