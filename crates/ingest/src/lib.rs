use std::path::PathBuf;
use std::time::Duration;

use buizz_core::api::{JobView, SpineEvent};
use buizz_core::market::Symbol;
use buizz_core::ops::JobOutcome;
use buizz_core::time::Timestamp;
use buizz_market::{MarketError, MarketStore};
use buizz_sources::Sources;
use buizz_sources::http::SourceError;
use buizz_state::{StateError, StateStore};
use chrono::NaiveDate;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    Market(#[from] MarketError),
    #[error(transparent)]
    State(#[from] StateError),
    #[error("{0} is not configured")]
    Unconfigured(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngestConfig {
    pub tick: Duration,
    pub snapshot_dir: PathBuf,
    pub snapshot_keep: usize,
    pub history_start: NaiveDate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobSpec {
    pub name: &'static str,
    pub every: Duration,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TickReport {
    pub ran: Vec<(&'static str, JobOutcome)>,
}

#[derive(Clone)]
pub struct Ingest {}

impl Ingest {
    pub fn new(
        _config: IngestConfig,
        _market: MarketStore,
        _state: StateStore,
        _sources: Sources,
    ) -> Self {
        unimplemented!()
    }

    pub fn jobs(&self) -> &[JobSpec] {
        unimplemented!()
    }

    pub async fn tick(&self, _now: Timestamp) -> Result<TickReport, IngestError> {
        unimplemented!()
    }

    pub async fn ensure_minute_coverage(
        &self,
        _symbols: &[Symbol],
        _from: NaiveDate,
        _to: NaiveDate,
    ) -> Result<(), IngestError> {
        unimplemented!()
    }

    pub async fn job_views(&self, _now: Timestamp) -> Result<Vec<JobView>, IngestError> {
        unimplemented!()
    }

    pub fn events(&self) -> broadcast::Receiver<SpineEvent> {
        unimplemented!()
    }

    pub fn spawn(&self) -> JoinHandle<()> {
        unimplemented!()
    }
}
