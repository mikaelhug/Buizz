use std::path::Path;

use buizz_core::api::RunSummary;
use buizz_core::ledger::{LedgerEntry, Portfolio};
use buizz_core::market::{DataSource, Dataset, Symbol};
use buizz_core::ops::{JobOutcome, JobRun, SourceFetch};
use buizz_core::order::{Fill, Order};
use buizz_core::performance::Metrics;
use buizz_core::run::{
    DesiredState, EquityPoint, Mode, RunBadge, RunConfig, RunEvent, RunId, RunStatus,
};
use buizz_core::time::Timestamp;
use rust_decimal::Decimal;

pub const SCHEMA: &str = include_str!("../schema.sql");
pub const FILE_NAME: &str = "state.db";

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("state store: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("state store encoding: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error("run {0} does not exist")]
    UnknownRun(RunId),
    #[error("step {step_seq} of run {run_id} is not the successor of the last committed step")]
    StaleStep { run_id: RunId, step_seq: u64 },
    #[error("state store has shut down")]
    Closed,
}

pub type Result<T> = std::result::Result<T, StateError>;

#[derive(Clone, Debug, PartialEq)]
pub struct NewRun {
    pub config: RunConfig,
    pub status: RunStatus,
    pub desired: DesiredState,
    pub bot_hash: String,
    pub engine_version: String,
    pub data_watermark: Option<Timestamp>,
    pub badges: Vec<RunBadge>,
    pub created_at: Timestamp,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunFilter {
    pub bot: Option<String>,
    pub mode: Option<Mode>,
    pub statuses: Vec<RunStatus>,
    pub desired: Option<DesiredState>,
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StepCommit {
    pub run_id: RunId,
    pub step_seq: u64,
    pub clock: Timestamp,
    pub equity: Decimal,
    pub ledger: Vec<LedgerEntry>,
    pub orders: Vec<Order>,
    pub equity_points: Vec<EquityPoint>,
    pub events: Vec<RunEvent>,
    pub engine_checkpoint: Vec<u8>,
    pub bot_state: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    pub step_seq: u64,
    pub clock: Timestamp,
    pub engine_checkpoint: Vec<u8>,
    pub bot_state: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunOutcome {
    pub status: RunStatus,
    pub finished_at: Timestamp,
    pub result_hash: Option<String>,
    pub metrics: Option<Metrics>,
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct StateStore {}

impl StateStore {
    pub async fn open(_path: &Path) -> Result<Self> {
        unimplemented!()
    }

    pub async fn create_run(&self, _run: NewRun) -> Result<RunId> {
        unimplemented!()
    }

    pub async fn run(&self, _run_id: RunId) -> Result<Option<RunSummary>> {
        unimplemented!()
    }

    pub async fn runs(&self, _filter: RunFilter) -> Result<Vec<RunSummary>> {
        unimplemented!()
    }

    pub async fn set_status(
        &self,
        _run_id: RunId,
        _status: RunStatus,
        _message: Option<String>,
        _at: Timestamp,
    ) -> Result<()> {
        unimplemented!()
    }

    pub async fn set_desired(&self, _run_id: RunId, _desired: DesiredState) -> Result<()> {
        unimplemented!()
    }

    pub async fn commit_step(&self, _commit: StepCommit) -> Result<()> {
        unimplemented!()
    }

    pub async fn checkpoint(&self, _run_id: RunId) -> Result<Option<Checkpoint>> {
        unimplemented!()
    }

    pub async fn finish_run(&self, _run_id: RunId, _outcome: RunOutcome) -> Result<()> {
        unimplemented!()
    }

    pub async fn append_events(&self, _run_id: RunId, _events: Vec<RunEvent>) -> Result<()> {
        unimplemented!()
    }

    pub async fn ledger(&self, _run_id: RunId) -> Result<Vec<LedgerEntry>> {
        unimplemented!()
    }

    pub async fn portfolio(&self, _run_id: RunId) -> Result<Portfolio> {
        unimplemented!()
    }

    pub async fn orders(
        &self,
        _run_id: RunId,
        _after: Option<u64>,
        _limit: u32,
    ) -> Result<Vec<Order>> {
        unimplemented!()
    }

    pub async fn fills(&self, _run_id: RunId) -> Result<Vec<Fill>> {
        unimplemented!()
    }

    pub async fn equity(&self, _run_id: RunId) -> Result<Vec<EquityPoint>> {
        unimplemented!()
    }

    pub async fn events(
        &self,
        _run_id: RunId,
        _after: Option<u64>,
        _limit: u32,
    ) -> Result<Vec<RunEvent>> {
        unimplemented!()
    }

    pub async fn holdout_runs(&self, _bot: &str) -> Result<u32> {
        unimplemented!()
    }

    pub async fn start_job(&self, _job: &str, _at: Timestamp) -> Result<i64> {
        unimplemented!()
    }

    pub async fn finish_job(
        &self,
        _job_run_id: i64,
        _at: Timestamp,
        _outcome: JobOutcome,
        _rows: u64,
        _message: Option<String>,
    ) -> Result<()> {
        unimplemented!()
    }

    pub async fn latest_jobs(&self) -> Result<Vec<JobRun>> {
        unimplemented!()
    }

    pub async fn last_job_success(&self, _job: &str) -> Result<Option<JobRun>> {
        unimplemented!()
    }

    pub async fn record_fetch(&self, _fetch: SourceFetch) -> Result<()> {
        unimplemented!()
    }

    pub async fn recent_fetches(
        &self,
        _source: DataSource,
        _dataset: Dataset,
        _symbol: Option<Symbol>,
        _limit: u32,
    ) -> Result<Vec<SourceFetch>> {
        unimplemented!()
    }

    pub async fn snapshot(&self, _destination: &Path) -> Result<()> {
        unimplemented!()
    }
}
