use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use buizz_core::time::Timestamp;
use tokio::sync::oneshot;

pub const UNMEASURED_RESERVATION: u64 = 2 << 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub backtests: usize,
    pub live_starts: usize,
    pub memory: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Ticket {
    Live(Timestamp, u64),
    Backtest(u64),
}

struct Waiter {
    memory: u64,
    grant: oneshot::Sender<()>,
}

struct Ledger {
    limits: Limits,
    backtests: usize,
    starting: usize,
    reserved: u64,
    live_peak: u64,
    issued: u64,
    live: BTreeMap<Ticket, Waiter>,
    queued: VecDeque<(Ticket, Waiter)>,
}

impl Ledger {
    fn fits(&self, memory: u64, room: u64) -> bool {
        self.limits
            .memory
            .is_none_or(|budget| self.reserved == 0 || self.reserved + memory + room <= budget)
    }

    fn dispatch(&mut self) {
        while let Some((&ticket, waiter)) = self.live.first_key_value() {
            let memory = waiter.memory;
            if !waiter.grant.is_closed()
                && (self.starting >= self.limits.live_starts || !self.fits(memory, 0))
            {
                break;
            }
            self.starting += 1;
            self.reserved += memory;
            let granted = self
                .live
                .remove(&ticket)
                .is_some_and(|waiter| waiter.grant.send(()).is_ok());
            if !granted {
                self.starting -= 1;
                self.reserved -= memory;
            }
        }
        if !self.live.is_empty() {
            return;
        }
        while let Some((_, waiter)) = self.queued.front() {
            let memory = waiter.memory;
            if !waiter.grant.is_closed()
                && (self.backtests >= self.limits.backtests || !self.fits(memory, self.live_peak))
            {
                break;
            }
            self.backtests += 1;
            self.reserved += memory;
            let Some((_, waiter)) = self.queued.pop_front() else {
                break;
            };
            if waiter.grant.send(()).is_err() {
                self.backtests -= 1;
                self.reserved -= memory;
            }
        }
    }

    fn withdraw(&mut self, ticket: Ticket) {
        match ticket {
            Ticket::Live(..) => {
                self.live.remove(&ticket);
            }
            Ticket::Backtest(_) => self.queued.retain(|(queued, _)| *queued != ticket),
        }
    }
}

#[derive(Clone)]
pub(crate) struct Admission(Arc<Mutex<Ledger>>);

impl Admission {
    pub(crate) fn new(limits: Limits) -> Self {
        Self(Arc::new(Mutex::new(Ledger {
            limits,
            backtests: 0,
            starting: 0,
            reserved: 0,
            live_peak: UNMEASURED_RESERVATION,
            issued: 0,
            live: BTreeMap::new(),
            queued: VecDeque::new(),
        })))
    }

    fn ledger(&self) -> MutexGuard<'_, Ledger> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn limits(&self) -> Limits {
        self.ledger().limits
    }

    async fn wait(&self, deadline: Option<Timestamp>, memory: u64) -> Ticket {
        let (grant, granted) = oneshot::channel();
        let ticket = {
            let mut ledger = self.ledger();
            ledger.issued += 1;
            let waiter = Waiter { memory, grant };
            let ticket = match deadline {
                Some(deadline) => {
                    ledger.live_peak = ledger.live_peak.max(memory);
                    let ticket = Ticket::Live(deadline, ledger.issued);
                    ledger.live.insert(ticket, waiter);
                    ticket
                }
                None => {
                    let ticket = Ticket::Backtest(ledger.issued);
                    ledger.queued.push_back((ticket, waiter));
                    ticket
                }
            };
            ledger.dispatch();
            ticket
        };
        let mut pending = Pending {
            admission: self.clone(),
            ticket,
            memory,
            granted: Some(granted),
        };
        if let Some(granted) = pending.granted.as_mut() {
            let _ = granted.await;
        }
        pending.granted = None;
        ticket
    }

    pub(crate) async fn backtest(&self, memory: u64) -> BacktestSlot {
        self.wait(None, memory).await;
        BacktestSlot {
            admission: self.clone(),
            memory,
        }
    }

    pub(crate) async fn live(&self, deadline: Timestamp, memory: u64) -> (LiveStart, Reservation) {
        self.wait(Some(deadline), memory).await;
        (
            LiveStart {
                admission: self.clone(),
            },
            Reservation {
                admission: self.clone(),
                memory,
            },
        )
    }
}

struct Pending {
    admission: Admission,
    ticket: Ticket,
    memory: u64,
    granted: Option<oneshot::Receiver<()>>,
}

impl Drop for Pending {
    fn drop(&mut self) {
        let Some(mut granted) = self.granted.take() else {
            return;
        };
        granted.close();
        let mut ledger = self.admission.ledger();
        if granted.try_recv().is_ok() {
            match self.ticket {
                Ticket::Live(..) => ledger.starting -= 1,
                Ticket::Backtest(_) => ledger.backtests -= 1,
            }
            ledger.reserved -= self.memory;
        } else {
            ledger.withdraw(self.ticket);
        }
        ledger.dispatch();
    }
}

pub struct BacktestSlot {
    admission: Admission,
    memory: u64,
}

impl Drop for BacktestSlot {
    fn drop(&mut self) {
        let mut ledger = self.admission.ledger();
        ledger.backtests -= 1;
        ledger.reserved -= self.memory;
        ledger.dispatch();
    }
}

pub struct LiveStart {
    admission: Admission,
}

impl Drop for LiveStart {
    fn drop(&mut self) {
        let mut ledger = self.admission.ledger();
        ledger.starting -= 1;
        ledger.dispatch();
    }
}

pub struct Reservation {
    admission: Admission,
    memory: u64,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut ledger = self.admission.ledger();
        ledger.reserved -= self.memory;
        ledger.dispatch();
    }
}
