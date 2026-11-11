mod actions;
mod bars;
mod facts;
mod fetch;
mod minute;
mod reference;
mod schedule;
mod snapshot;
mod tiingo;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use buizz_core::api::{JobView, SpineEvent};
use buizz_core::market::{Coverage, DataSource, Dataset, Instrument, Symbol};
use buizz_core::ops::JobOutcome;
use buizz_core::time::Timestamp;
use buizz_market::{MarketError, MarketStore};
use buizz_sources::Sources;
use buizz_sources::http::SourceError;
use buizz_state::{StateError, StateStore};
use chrono::{NaiveDate, NaiveTime, TimeDelta};
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;

use crate::fetch::Work;
use crate::schedule::{JOBS, due_at};

const EVENT_CAPACITY: usize = 64;

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
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngestConfig {
    pub tick: Duration,
    pub snapshot_dir: PathBuf,
    pub snapshot_keep: usize,
    pub history_start: NaiveDate,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct JobSpec {
    pub name: &'static str,
    pub every: Duration,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TickReport {
    pub ran: Vec<(&'static str, JobOutcome)>,
}

#[derive(Clone)]
pub struct Ingest {
    config: IngestConfig,
    market: MarketStore,
    state: StateStore,
    sources: Sources,
    events: broadcast::Sender<SpineEvent>,
    gate: Arc<Mutex<()>>,
}

impl Ingest {
    pub fn new(
        config: IngestConfig,
        market: MarketStore,
        state: StateStore,
        sources: Sources,
    ) -> Self {
        Self {
            config,
            market,
            state,
            sources,
            events: broadcast::Sender::new(EVENT_CAPACITY),
            gate: Arc::new(Mutex::new(())),
        }
    }

    pub async fn tick(&self, now: Timestamp) -> Result<TickReport, IngestError> {
        let _serial = self.gate.lock().await;
        let started = tokio::time::Instant::now();
        let at = || now + TimeDelta::from_std(started.elapsed()).unwrap_or_default();
        let latest = self.state.latest_jobs().await?;
        let failures = self.state.failure_streaks().await?;
        let mut report = TickReport::default();
        for spec in JOBS {
            let previous = latest.iter().find(|run| run.job == spec.name);
            let streak = failures.get(spec.name).copied().unwrap_or(0);
            if due_at(&spec, previous, streak).is_some_and(|due| due > at()) {
                continue;
            }
            let run_id = self.state.start_job(spec.name, at()).await?;
            let work = match self.run(spec.name, at()).await {
                Ok(work) => work,
                Err(error) => Work::failed(error.to_string()),
            };
            if work.outcome == JobOutcome::Failed {
                tracing::error!(job = spec.name, message = %work.message, "ingest job failed");
            }
            let finished = at();
            self.state
                .finish_job(
                    run_id,
                    finished,
                    work.outcome,
                    work.rows,
                    Some(work.message),
                )
                .await?;
            report.ran.push((spec.name, work.outcome));
            let views = self.job_views(finished).await?;
            if let Some(job) = views.into_iter().find(|view| view.name == spec.name) {
                let _ = self.events.send(SpineEvent::Job { job });
            }
        }
        Ok(report)
    }

    pub async fn job_views(&self, now: Timestamp) -> Result<Vec<JobView>, IngestError> {
        let latest = self.state.latest_jobs().await?;
        let failures = self.state.failure_streaks().await?;
        let mut views = Vec::with_capacity(JOBS.len());
        for spec in JOBS {
            let previous = latest.iter().find(|run| run.job == spec.name);
            let streak = failures.get(spec.name).copied().unwrap_or(0);
            let success = self.state.last_job_success(spec.name).await?;
            let view = schedule::view(&spec, previous, streak, success.as_ref(), now);
            views.push(JobView {
                stale: self.config.enabled && view.stale,
                ..view
            });
        }
        Ok(views)
    }

    pub fn events(&self) -> broadcast::Receiver<SpineEvent> {
        self.events.subscribe()
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    pub fn spawn(&self) -> Option<JoinHandle<()>> {
        if !self.config.enabled {
            return None;
        }
        let ingest = self.clone();
        Some(tokio::spawn(async move {
            if let Err(error) = ingest.restore_daily_watermark().await {
                tracing::error!(%error, "could not restore the daily watermark");
            }
            let mut interval = tokio::time::interval(ingest.config.tick);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if let Err(error) = ingest.tick(Timestamp::now()).await {
                    tracing::error!(%error, "ingest tick failed");
                }
            }
        }))
    }

    async fn run(&self, job: &str, now: Timestamp) -> Result<Work, IngestError> {
        match job {
            schedule::CALENDAR => self.calendar(now).await,
            schedule::INSTRUMENTS => self.instruments(now).await,
            schedule::BARS_1D => self.daily_bars(now).await,
            schedule::CORPORATE_ACTIONS => self.corporate_actions(now).await,
            schedule::FUNDAMENTALS => self.fundamentals(now).await,
            schedule::RATES => self.rates(now).await,
            schedule::STATE_SNAPSHOT => self.state_snapshot(now).await,
            schedule::TIINGO_HISTORY => self.tiingo_history(now).await,
            other => unreachable!("unscheduled job {other}"),
        }
    }

    async fn active_instruments(&self) -> Result<Vec<Instrument>, IngestError> {
        let instruments = self.market.instruments().await?;
        Ok(instruments
            .into_iter()
            .filter(|instrument| instrument.active)
            .collect())
    }

    async fn active_symbols(&self) -> Result<Vec<Symbol>, IngestError> {
        Ok(self
            .active_instruments()
            .await?
            .into_iter()
            .map(|instrument| instrument.symbol)
            .collect())
    }

    async fn symbol_coverage(
        &self,
        dataset: Dataset,
        source: DataSource,
    ) -> Result<BTreeMap<Symbol, Coverage>, IngestError> {
        let rows = self.market.coverage().await?;
        Ok(rows
            .into_iter()
            .filter(|row| row.dataset == dataset && row.source == source)
            .filter_map(|row| Some((row.symbol.clone()?, row)))
            .collect())
    }
}

pub(crate) fn day_start(date: NaiveDate) -> Timestamp {
    Timestamp::at_exchange_time(date, NaiveTime::MIN).expect("midnight exists in New York")
}
