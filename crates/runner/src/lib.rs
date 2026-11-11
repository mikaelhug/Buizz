use std::path::PathBuf;

use axum::extract::ws::WebSocket;
use buizz_core::api::{RunSummary, SpineEvent, StartBacktest};
use buizz_core::ops::CoverageGap;
use buizz_core::run::{CostModel, RiskLimits, RunId};
use buizz_feed::{FeedError, FeedManager};
use buizz_ingest::{Ingest, IngestError};
use buizz_market::{MarketError, MarketStore};
use buizz_state::{StateError, StateStore};
use buizz_supervisor::{BotScan, Supervisor, SupervisorError};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use tokio::sync::broadcast;

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("unknown bot {0}")]
    UnknownBot(String),
    #[error("unknown run {0}")]
    UnknownRun(RunId),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("the period touches the locked holdout; pass allow_holdout to run on it")]
    HoldoutLocked,
    #[error("market data has gaps")]
    CoverageGaps(Vec<CoverageGap>),
    #[error("bot {0} already has a live run")]
    AlreadyLive(String),
    #[error("bot {0} has no live run")]
    NotLive(String),
    #[error(transparent)]
    Feed(#[from] FeedError),
    #[error(transparent)]
    Ingest(#[from] IngestError),
    #[error(transparent)]
    Market(#[from] MarketError),
    #[error(transparent)]
    State(#[from] StateError),
    #[error(transparent)]
    Supervisor(#[from] SupervisorError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunnerConfig {
    pub bots_dir: PathBuf,
    pub start_cash: Decimal,
    pub cost: CostModel,
    pub risk: RiskLimits,
}

pub struct RunnerDeps {
    pub market: MarketStore,
    pub state: StateStore,
    pub supervisor: Supervisor,
    pub feed: FeedManager,
    pub ingest: Ingest,
}

#[derive(Clone)]
pub struct Runner {}

impl Runner {
    pub fn new(_config: RunnerConfig, _deps: RunnerDeps) -> Self {
        unimplemented!()
    }

    pub fn bots(&self) -> BotScan {
        unimplemented!()
    }

    pub async fn recover(&self) -> Result<(), RunnerError> {
        unimplemented!()
    }

    pub async fn start_backtest(
        &self,
        _bot: &str,
        _request: StartBacktest,
        _today: NaiveDate,
    ) -> Result<RunSummary, RunnerError> {
        unimplemented!()
    }

    pub async fn go_live(&self, _bot: &str) -> Result<RunSummary, RunnerError> {
        unimplemented!()
    }

    pub async fn pause(&self, _bot: &str) -> Result<RunSummary, RunnerError> {
        unimplemented!()
    }

    pub async fn cancel(&self, _run_id: RunId) -> Result<RunSummary, RunnerError> {
        unimplemented!()
    }

    pub fn authorize(&self, _token: &str) -> Option<RunId> {
        unimplemented!()
    }

    pub async fn attach_bot(&self, _run_id: RunId, _socket: WebSocket) {
        unimplemented!()
    }

    pub fn events(&self) -> broadcast::Receiver<SpineEvent> {
        unimplemented!()
    }

    pub async fn shutdown(&self) {
        unimplemented!()
    }
}
