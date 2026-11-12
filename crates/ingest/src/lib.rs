mod actions;
mod bars;
mod borrow;
mod estimates;
mod facts;
mod fetch;
mod filings;
mod fomc;
mod index;
mod insiders;
mod macros;
mod minute;
mod news;
mod reference;
mod schedule;
mod sec_index;
mod snapshot;
mod tiingo;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use buizz_core::api::{BackfillProgress, JobView, SpineEvent};
use buizz_core::market::{
    AssetClass, Coverage, DataSource, Dataset, Instrument, StockIndex, Symbol,
};
use buizz_core::ops::JobOutcome;
use buizz_core::time::Timestamp;
use buizz_market::{MarketError, MarketStore, Restored};
use buizz_sources::Sources;
use buizz_sources::http::SourceError;
use buizz_state::{StateError, StateStore};
use chrono::{NaiveDate, NaiveTime, TimeDelta};
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;

use crate::fetch::Work;
use crate::schedule::{JOBS, due_at};

const EVENT_CAPACITY: usize = 64;
const OTC: &str = "OTC";
const CORE_ETFS: [&str; 40] = [
    "SPY", "QQQ", "IWM", "DIA", "IVV", "VOO", "VTI", "MDY", "IJR", "EFA", "EEM", "VEU", "VWO",
    "AGG", "BND", "TLT", "IEF", "SHY", "SHV", "BIL", "LQD", "HYG", "TIP", "GLD", "SLV", "DBC",
    "GSG", "USO", "VNQ", "XLB", "XLC", "XLE", "XLF", "XLI", "XLK", "XLP", "XLRE", "XLU", "XLV",
    "XLY",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Priced {
    pub symbol: Symbol,
    pub rank: usize,
    pub active: bool,
}

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
    pub only: Option<BTreeSet<Symbol>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct JobSpec {
    pub name: &'static str,
    pub source: Option<DataSource>,
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
    ran_since_boot: Arc<Mutex<BTreeSet<&'static str>>>,
    stopping: Arc<AtomicBool>,
    booted_at: Timestamp,
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
            ran_since_boot: Arc::default(),
            stopping: Arc::default(),
            booted_at: Timestamp::now(),
        }
    }

    pub async fn tick(&self, now: Timestamp) -> Result<TickReport, IngestError> {
        let mut ran_since_boot = self.ran_since_boot.lock().await;
        let started = tokio::time::Instant::now();
        let finished_at = || now + TimeDelta::from_std(started.elapsed()).unwrap_or_default();
        let latest = self.state.latest_jobs().await?;
        let failures = self.state.failure_streaks(self.booted_at).await?;
        let limits = self.rate_limits().await?;
        let mut report = TickReport::default();
        for spec in JOBS {
            if self.stopping() {
                break;
            }
            let previous = latest.iter().find(|run| run.job == spec.name);
            let streak = failures.get(spec.name).copied().unwrap_or(0);
            let limited = spec.source.and_then(|source| limits.get(&source).copied());
            let waiting = due_at(&spec, previous, streak, limited).is_some_and(|due| due > now);
            let settled_before_boot = !ran_since_boot.contains(spec.name)
                && previous.is_some_and(|run| {
                    matches!(run.outcome, Some(JobOutcome::Ok | JobOutcome::Noop))
                });
            if waiting && !(settled_before_boot && self.series_unread(spec.name).await?) {
                continue;
            }
            ran_since_boot.insert(spec.name);
            let run_id = self.state.start_job(spec.name, now).await?;
            let work = match self.run(spec.name, now).await {
                Ok(work) => work,
                Err(error) => Work::failed(error.to_string()),
            };
            if work.outcome == JobOutcome::Failed {
                tracing::error!(job = spec.name, message = %work.message, "ingest job failed");
            }
            let finished = finished_at();
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
        let failures = self.state.failure_streaks(self.booted_at).await?;
        let limits = self.rate_limits().await?;
        let mut views = Vec::with_capacity(JOBS.len());
        for spec in JOBS {
            let previous = latest.iter().find(|run| run.job == spec.name);
            let streak = failures.get(spec.name).copied().unwrap_or(0);
            let limited = spec.source.and_then(|source| limits.get(&source).copied());
            let success = self.state.last_job_success(spec.name).await?;
            let view = schedule::view(&spec, previous, streak, limited, success.as_ref(), now);
            views.push(JobView {
                stale: self.config.enabled && view.stale,
                ..view
            });
        }
        Ok(views)
    }

    async fn rate_limits(&self) -> Result<BTreeMap<DataSource, Timestamp>, IngestError> {
        let mut limits = BTreeMap::new();
        for source in JOBS.iter().filter_map(|spec| spec.source) {
            if let Some(until) = self.state.rate_limited_until(source).await? {
                limits.insert(source, until);
            }
        }
        Ok(limits)
    }

    pub fn events(&self) -> broadcast::Receiver<SpineEvent> {
        self.events.subscribe()
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    pub async fn stop(&self, grace: Duration) {
        self.stopping.store(true, Ordering::SeqCst);
        if tokio::time::timeout(grace, self.ran_since_boot.lock())
            .await
            .is_err()
        {
            tracing::warn!(
                ?grace,
                "an ingest job was still running at shutdown; it resumes on the next start"
            );
        }
    }

    pub(crate) fn stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    pub fn spawn(&self) -> Option<JoinHandle<()>> {
        if !self.config.enabled {
            return None;
        }
        let ingest = self.clone();
        Some(tokio::spawn(async move {
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
            schedule::SEC_INDEX => self.sec_index(now).await,
            schedule::FUNDAMENTALS => self.fundamentals(now).await,
            schedule::FILINGS => self.filings(now).await,
            schedule::INSIDER_TRADES => self.insider_trades(now).await,
            schedule::NEWS => self.news(now).await,
            schedule::INDEX_MEMBERSHIP => self.index_membership(now).await,
            schedule::RATES => self.rates(now).await,
            schedule::BORROW => self.borrow(now).await,
            schedule::MACRO => self.macro_series(now).await,
            schedule::VIX => self.vix(now).await,
            schedule::FOMC => self.fomc(now).await,
            schedule::EARNINGS_INSIGHT => self.earnings_insight(now).await,
            schedule::EARNINGS_INSIGHT_ARCHIVE => self.earnings_insight_archive(now).await,
            schedule::STATE_SNAPSHOT => self.state_snapshot(now).await,
            schedule::MARKET_SNAPSHOT => self.market_snapshot(now).await,
            schedule::TIINGO_HISTORY => self.tiingo_history(now).await,
            other => unreachable!("unscheduled job {other}"),
        }
    }

    pub async fn backfill_progress(&self) -> Result<Vec<BackfillProgress>, IngestError> {
        let priced = self.priced_symbols().await?;
        let coverage = self.market.coverage().await?;
        let covered = |dataset: Dataset, source: DataSource| -> BTreeMap<&Symbol, &Coverage> {
            coverage
                .iter()
                .filter(|row| row.dataset == dataset && row.source == source)
                .filter_map(|row| Some((row.symbol.as_ref()?, row)))
                .collect()
        };
        let count = |symbols: &mut dyn Iterator<Item = bool>| {
            let (mut done, mut total) = (0_u32, 0_u32);
            for settled in symbols {
                total += 1;
                done += u32::from(settled);
            }
            (done, total)
        };
        let mut progress = Vec::new();
        let mut push = |dataset, source, (done, total): (u32, u32)| {
            progress.push(BackfillProgress {
                dataset,
                source,
                done,
                total,
            });
        };
        let bars = covered(Dataset::Bars1d, DataSource::Alpaca);
        let checked = self
            .state
            .settled_symbols(DataSource::Alpaca, Dataset::Bars1d)
            .await?;
        push(
            Dataset::Bars1d,
            DataSource::Alpaca,
            count(
                &mut priced
                    .iter()
                    .map(|p| bars.contains_key(&p.symbol) || checked.contains(&p.symbol)),
            ),
        );
        let actions = covered(Dataset::CorporateActions, DataSource::Alpaca);
        let history_start = self.config.history_start;
        push(
            Dataset::CorporateActions,
            DataSource::Alpaca,
            count(&mut priced.iter().map(|p| {
                actions
                    .get(&p.symbol)
                    .is_some_and(|row| row.first <= history_start)
            })),
        );
        let priced_set: BTreeSet<&Symbol> = priced.iter().map(|p| &p.symbol).collect();
        let stocks: BTreeSet<Symbol> = self
            .market
            .instruments()
            .await?
            .into_iter()
            .filter(|instrument| instrument.active && instrument.asset_class == AssetClass::Stock)
            .map(|instrument| instrument.symbol)
            .filter(|symbol| priced_set.contains(symbol))
            .collect();
        for dataset in [Dataset::Fundamentals, Dataset::Companies] {
            let read = self
                .state
                .settled_symbols(DataSource::Edgar, dataset)
                .await?;
            push(
                dataset,
                DataSource::Edgar,
                count(&mut stocks.iter().map(|symbol| read.contains(symbol))),
            );
        }
        if let Some(floor) = bars.values().map(|row| row.first).min() {
            let extended = self
                .state
                .settled_symbols(DataSource::Tiingo, Dataset::Bars1d)
                .await?;
            let calendar = self.market.calendar(history_start, floor).await?;
            let candidates = priced.iter().filter(|p| {
                bars.get(&p.symbol).is_some_and(|row| row.first == floor)
                    && calendar
                        .sessions()
                        .first()
                        .is_some_and(|first| first.date < floor)
            });
            push(
                Dataset::Bars1d,
                DataSource::Tiingo,
                count(&mut candidates.map(|p| extended.contains(&p.symbol))),
            );
        }
        Ok(progress)
    }

    async fn delivered_symbols(&self) -> Result<BTreeSet<Symbol>, IngestError> {
        Ok(self
            .market
            .actions_reaching_other_symbols()
            .await?
            .iter()
            .filter_map(|action| action.kind.delivered().cloned())
            .collect())
    }

    async fn priced_symbols(&self) -> Result<Vec<Priced>, IngestError> {
        let mut members: BTreeMap<Symbol, bool> = self
            .market
            .index_history(StockIndex::Sp500)
            .await?
            .map(|history| history.members)
            .unwrap_or_default()
            .into_iter()
            .fold(BTreeMap::new(), |mut members, member| {
                *members.entry(member.symbol).or_default() |= member.end.is_none();
                members
            });
        for symbol in self.delivered_symbols().await? {
            members.entry(symbol).or_default();
        }
        let instruments: BTreeMap<Symbol, Instrument> = self
            .market
            .instruments()
            .await?
            .into_iter()
            .filter(|instrument| instrument.exchange != OTC)
            .map(|instrument| (instrument.symbol.clone(), instrument))
            .collect();
        let symbols: BTreeSet<&Symbol> = instruments
            .keys()
            .chain(members.keys())
            .filter(|symbol| {
                self.config
                    .only
                    .as_ref()
                    .is_none_or(|only| only.contains(*symbol))
            })
            .collect();
        let mut priced: Vec<Priced> = symbols
            .into_iter()
            .map(|symbol| {
                let instrument = instruments.get(symbol);
                let rank = match (
                    CORE_ETFS.iter().position(|core| *core == symbol.as_str()),
                    members.get(symbol),
                ) {
                    (Some(core), _) => core,
                    (None, Some(true)) => CORE_ETFS.len(),
                    (None, Some(false)) => CORE_ETFS.len() + 1,
                    (None, None) if instrument.is_some_and(|known| known.active) => {
                        CORE_ETFS.len() + 2
                    }
                    (None, None) => CORE_ETFS.len() + 3,
                };
                Priced {
                    symbol: symbol.clone(),
                    rank,
                    active: instrument.is_some_and(|known| known.active),
                }
            })
            .collect();
        priced.sort_by(|a, b| (a.rank, &a.symbol).cmp(&(b.rank, &b.symbol)));
        Ok(priced)
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

pub async fn restore(
    market: &MarketStore,
    state: &StateStore,
    backup: PathBuf,
    at: Timestamp,
) -> Result<Restored, IngestError> {
    let restored = market.restore(backup).await?;
    state.reset_market(at).await?;
    market.checkpoint().await?;
    Ok(restored)
}

pub(crate) fn day_start(date: NaiveDate) -> Timestamp {
    Timestamp::at_exchange_time(date, NaiveTime::MIN).expect("midnight exists in New York")
}
