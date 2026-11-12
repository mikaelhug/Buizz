mod codec;
mod db;
mod jobs;
mod runs;

use std::collections::{BTreeMap, BTreeSet};
use std::ops::RangeInclusive;
use std::path::Path;
use std::sync::Arc;

use buizz_core::api::{RunSummary, SparkPoint};
use buizz_core::ledger::{LedgerEntry, Portfolio};
use buizz_core::market::{DataSource, Dataset, Symbol};
use buizz_core::ops::{JobOutcome, JobRun, SourceFetch};
use buizz_core::order::{Fill, Order, OrderId};
use buizz_core::performance::Metrics;
use buizz_core::run::{
    DesiredState, EquityPoint, Mode, RunBadge, RunConfig, RunEvent, RunId, RunStatus,
};
use buizz_core::time::Timestamp;
use rust_decimal::Decimal;

use crate::db::Db;

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
    #[error("bot {0} already has a live run")]
    LiveRunExists(String),
    #[error("state store schema is version {found}, newer than the {known} this spine knows")]
    NewerSchema { found: i64, known: i64 },
    #[error("state store has shut down")]
    Closed,
}

pub type Result<T> = std::result::Result<T, StateError>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FetchUsage {
    pub requests: u64,
    pub symbols: u64,
}

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
    pub before: Option<RunId>,
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
    pub undelivered: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    pub step_seq: u64,
    pub clock: Timestamp,
    pub engine_checkpoint: Vec<u8>,
    pub bot_state: Option<Vec<u8>>,
    pub undelivered: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunOutcome {
    pub status: RunStatus,
    pub finished_at: Timestamp,
    pub result_hash: Option<String>,
    pub metrics: Option<Metrics>,
    pub message: Option<String>,
    pub badge: Option<RunBadge>,
}

#[derive(Clone)]
pub struct StateStore {
    db: Arc<Db>,
}

impl StateStore {
    pub async fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            db: Arc::new(Db::open(path).await?),
        })
    }

    pub async fn create_run(&self, run: NewRun) -> Result<RunId> {
        self.db
            .transaction(move |transaction| runs::create(transaction, &run))
            .await
    }

    pub async fn run(&self, run_id: RunId) -> Result<Option<RunSummary>> {
        self.db
            .read(move |connection| runs::get(connection, run_id))
            .await
    }

    pub async fn runs(&self, filter: RunFilter) -> Result<Vec<RunSummary>> {
        self.db
            .read(move |connection| runs::list(connection, &filter))
            .await
    }

    pub async fn set_status(
        &self,
        run_id: RunId,
        status: RunStatus,
        message: Option<String>,
        at: Timestamp,
    ) -> Result<()> {
        self.db
            .transaction(move |transaction| {
                runs::set_status(transaction, run_id, status, message.as_deref(), at)
            })
            .await
    }

    pub async fn set_desired(&self, run_id: RunId, desired: DesiredState) -> Result<()> {
        self.db
            .transaction(move |transaction| runs::set_desired(transaction, run_id, desired))
            .await
    }

    pub async fn commit_step(&self, commit: StepCommit) -> Result<()> {
        self.db
            .transaction(move |transaction| runs::commit_step(transaction, &commit))
            .await
    }

    pub async fn checkpoint(&self, run_id: RunId) -> Result<Option<Checkpoint>> {
        self.db
            .read(move |connection| runs::checkpoint(connection, run_id))
            .await
    }

    pub async fn finish_run(&self, run_id: RunId, outcome: RunOutcome) -> Result<()> {
        self.db
            .transaction(move |transaction| runs::finish(transaction, run_id, &outcome))
            .await
    }

    pub async fn append_events(&self, run_id: RunId, events: Vec<RunEvent>) -> Result<()> {
        self.db
            .transaction(move |transaction| runs::insert_events(transaction, run_id, &events))
            .await
    }

    pub async fn ledger(&self, run_id: RunId) -> Result<Vec<LedgerEntry>> {
        self.db
            .read(move |connection| runs::ledger(connection, run_id))
            .await
    }

    pub async fn portfolio(&self, run_id: RunId) -> Result<Portfolio> {
        self.db
            .read(move |connection| runs::portfolio(connection, run_id))
            .await
    }

    pub async fn orders(
        &self,
        run_id: RunId,
        after: Option<u64>,
        limit: u32,
    ) -> Result<Vec<Order>> {
        self.db
            .read(move |connection| runs::orders(connection, run_id, after, limit))
            .await
    }

    pub async fn fills(&self, run_id: RunId, orders: RangeInclusive<OrderId>) -> Result<Vec<Fill>> {
        self.db
            .read(move |connection| runs::fills(connection, run_id, orders))
            .await
    }

    pub async fn equity(&self, run_id: RunId) -> Result<Vec<EquityPoint>> {
        self.db
            .read(move |connection| runs::equity(connection, run_id))
            .await
    }

    pub async fn sparkline(&self, run_id: RunId, points: u32) -> Result<Vec<SparkPoint>> {
        self.db
            .read(move |connection| runs::sparkline(connection, run_id, points))
            .await
    }

    pub async fn events(
        &self,
        run_id: RunId,
        after: Option<u64>,
        limit: u32,
    ) -> Result<Vec<RunEvent>> {
        self.db
            .read(move |connection| runs::events(connection, run_id, after, limit))
            .await
    }

    pub async fn holdout_runs(&self, bot: &str) -> Result<u32> {
        let bot = bot.to_owned();
        self.db
            .read(move |connection| runs::holdout_runs(connection, &bot))
            .await
    }

    pub async fn start_job(&self, job: &str, at: Timestamp) -> Result<i64> {
        let job = job.to_owned();
        self.db
            .transaction(move |transaction| jobs::start_job(transaction, &job, at))
            .await
    }

    pub async fn finish_job(
        &self,
        job_run_id: i64,
        at: Timestamp,
        outcome: JobOutcome,
        rows: u64,
        message: Option<String>,
    ) -> Result<()> {
        self.db
            .transaction(move |transaction| {
                jobs::finish_job(
                    transaction,
                    job_run_id,
                    at,
                    outcome,
                    rows,
                    message.as_deref(),
                )
            })
            .await
    }

    pub async fn latest_jobs(&self) -> Result<Vec<JobRun>> {
        self.db.read(jobs::latest_jobs).await
    }

    pub async fn last_job_success(&self, job: &str) -> Result<Option<JobRun>> {
        let job = job.to_owned();
        self.db
            .read(move |connection| jobs::last_job_success(connection, &job))
            .await
    }

    pub async fn failure_streaks(
        &self,
        unfinished_before: Timestamp,
    ) -> Result<BTreeMap<String, u32>> {
        self.db
            .read(move |connection| jobs::failure_streaks(connection, unfinished_before))
            .await
    }

    pub async fn rate_limited_until(&self, source: DataSource) -> Result<Option<Timestamp>> {
        self.db
            .read(move |connection| jobs::rate_limited_until(connection, source))
            .await
    }

    pub async fn reset_market(&self, at: Timestamp) -> Result<bool> {
        self.db
            .transaction(move |transaction| jobs::reset_market(transaction, at))
            .await
    }

    pub async fn record_fetch(&self, fetch: SourceFetch) -> Result<()> {
        self.db
            .transaction(move |transaction| jobs::record_fetch(transaction, &fetch))
            .await
    }

    pub async fn fetch_usage(&self, source: DataSource, since: Timestamp) -> Result<FetchUsage> {
        self.db
            .read(move |connection| jobs::fetch_usage(connection, source, since))
            .await
    }

    pub async fn recent_fetches(
        &self,
        source: DataSource,
        dataset: Dataset,
        symbol: Option<Symbol>,
        limit: u32,
    ) -> Result<Vec<SourceFetch>> {
        self.db
            .read(move |connection| {
                jobs::recent_fetches(connection, source, dataset, symbol.as_ref(), limit)
            })
            .await
    }

    pub async fn settled_symbols(
        &self,
        source: DataSource,
        dataset: Dataset,
    ) -> Result<BTreeSet<Symbol>> {
        self.db
            .read(move |connection| jobs::settled_symbols(connection, source, dataset))
            .await
    }

    pub async fn latest_rejections(
        &self,
        source: DataSource,
        dataset: Dataset,
    ) -> Result<Vec<SourceFetch>> {
        self.db
            .read(move |connection| jobs::latest_rejections(connection, source, dataset))
            .await
    }

    pub async fn snapshot(&self, destination: &Path) -> Result<()> {
        let destination = destination.to_path_buf();
        self.db
            .read(move |connection| {
                connection.execute("VACUUM INTO ?", [destination.to_string_lossy()])?;
                Ok(())
            })
            .await
    }
}
