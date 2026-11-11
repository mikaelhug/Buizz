use std::collections::BTreeMap;

use buizz_core::ledger::{LedgerEntry, Portfolio};
use buizz_core::market::{Bar, CorporateAction, Session, Symbol};
use buizz_core::order::{Fill, Order, OrderRequest};
use buizz_core::protocol::{Holdings, StepEvent};
use buizz_core::run::{EquityPoint, RiskLimit, RunConfig};
use buizz_core::time::Timestamp;
use chrono::NaiveDate;
use rust_decimal::Decimal;

pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("engine checkpoint is unreadable: {0}")]
    Checkpoint(#[from] rmp_serde::decode::Error),
    #[error("engine checkpoint was written by engine {0}")]
    Version(String),
}

pub struct Cycle<'a> {
    pub session: &'a Session,
    pub start: Timestamp,
    pub visible_at: Timestamp,
    pub bars: &'a [Bar],
    pub actions: &'a [CorporateAction],
    pub average_daily_volume: &'a BTreeMap<Symbol, u64>,
    pub delisted: &'a [Symbol],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiskBreach {
    pub limit: RiskLimit,
    pub value: Decimal,
    pub threshold: Decimal,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outbox {
    pub ledger: Vec<LedgerEntry>,
    pub orders: Vec<Order>,
    pub fills: Vec<Fill>,
    pub step_events: Vec<StepEvent>,
    pub equity_points: Vec<EquityPoint>,
    pub breaches: Vec<RiskBreach>,
}

pub struct Engine {}

impl Engine {
    pub fn new(_config: &RunConfig, _start: Timestamp) -> Self {
        unimplemented!()
    }

    pub fn restore(_config: &RunConfig, _checkpoint: &[u8]) -> Result<Self, EngineError> {
        unimplemented!()
    }

    pub fn checkpoint(&self) -> Vec<u8> {
        unimplemented!()
    }

    pub fn run_cycle(&mut self, _cycle: &Cycle<'_>) {
        unimplemented!()
    }

    pub fn accept(
        &mut self,
        _decided_at: Timestamp,
        _session: NaiveDate,
        _orders: Vec<OrderRequest>,
        _cancels: Vec<String>,
    ) {
        unimplemented!()
    }

    pub fn cancel_all(&mut self, _at: Timestamp) {
        unimplemented!()
    }

    pub fn drain(&mut self) -> Outbox {
        unimplemented!()
    }

    pub fn portfolio(&self) -> &Portfolio {
        unimplemented!()
    }

    pub fn holdings(&self) -> Holdings {
        unimplemented!()
    }

    pub fn peak_equity(&self) -> Decimal {
        unimplemented!()
    }

    pub fn result_hash(&self) -> String {
        unimplemented!()
    }
}
