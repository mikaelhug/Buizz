mod actions;
mod bars;
mod borrow;
mod contract;
mod crosscheck;
mod curated;
mod delistings;
mod documents;
mod estimates;
mod etfs;
mod facts;
mod fails;
mod fetch;
mod filings;
mod finra;
mod fomc;
mod halts;
mod index;
mod insiders;
mod macros;
mod minute;
mod news;
mod nordic;
mod options;
mod press;
pub mod recipe;
mod reference;
mod schedule;
mod sec_index;
mod sections;
mod snapshot;
mod tiingo;
mod views;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use buizz_core::api::{CoverageContract, JobView, SpineEvent};
use buizz_core::market::{Coverage, DataSource, Dataset, Instrument, StockIndex, Symbol};
use buizz_core::ops::{JobOutcome, JobRun};
use buizz_core::time::Timestamp;
use buizz_market::{MarketError, MarketStore, Restored};
use buizz_sources::Sources;
use buizz_sources::http::SourceError;
use buizz_state::{StateError, StateStore};
use chrono::{NaiveDate, NaiveTime, TimeDelta};
use tokio::sync::{Mutex, RwLock, broadcast, watch};
use tokio::task::JoinHandle;

use crate::fetch::Work;
use crate::schedule::{JOBS, due_at};

const EVENT_CAPACITY: usize = 256;
const SETTLE_LOOKAHEAD_DAYS: i64 = 10;
const OTC: &str = "OTC";
pub(crate) const CORE_ETFS: [&str; 63] = [
    "SPY", "QQQ", "IWM", "DIA", "IVV", "VOO", "VTI", "MDY", "IJR", "EFA", "EEM", "VEU", "VWO",
    "AGG", "BND", "TLT", "IEF", "SHY", "SHV", "BIL", "LQD", "HYG", "TIP", "GLD", "SLV", "DBC",
    "GSG", "USO", "VNQ", "XLB", "XLC", "XLE", "XLF", "XLI", "XLK", "XLP", "XLRE", "XLU", "XLV",
    "XLY", "MTUM", "QUAL", "USMV", "VLUE", "IWD", "IWF", "IWN", "IWO", "EWJ", "FXI", "EWZ", "EZU",
    "IEI", "EMB", "MUB", "DBA", "UNG", "UUP", "FXE", "FXY", "SSO", "QLD", "UPRO",
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
    #[error("recipe: {0}")]
    Recipe(String),
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
    pub datasets: &'static [Dataset],
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
    pending_seen: Arc<Mutex<BTreeMap<&'static str, u64>>>,
    running: Arc<RwLock<()>>,
    stopping: Arc<AtomicBool>,
    booted_at: Timestamp,
    contracts: Arc<watch::Sender<Vec<CoverageContract>>>,
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
            pending_seen: Arc::default(),
            running: Arc::default(),
            stopping: Arc::default(),
            booted_at: Timestamp::now(),
            contracts: Arc::new(watch::Sender::new(Vec::new())),
        }
    }

    pub async fn tick(&self, now: Timestamp) -> Result<TickReport, IngestError> {
        let every: Vec<&'static JobSpec> = JOBS.iter().collect();
        self.tick_lane(&every, now).await
    }

    async fn tick_lane(
        &self,
        lane: &[&'static JobSpec],
        now: Timestamp,
    ) -> Result<TickReport, IngestError> {
        let started = tokio::time::Instant::now();
        let finished_at = || now + TimeDelta::from_std(started.elapsed()).unwrap_or_default();
        let latest = self.state.latest_jobs().await?;
        let failures = self.state.failure_streaks(self.booted_at).await?;
        let limits = self.rate_limits().await?;
        let mut report = TickReport::default();
        let mut lane = lane.to_vec();
        lane.sort_by_key(|spec| {
            latest
                .iter()
                .find(|run| run.job == spec.name)
                .map(|run| run.started_at)
        });
        for spec in lane {
            if self.stopping() {
                break;
            }
            let running = self.running.read().await;
            let previous = latest.iter().find(|run| run.job == spec.name);
            let streak = failures.get(spec.name).copied().unwrap_or(0);
            let limited = spec.source.and_then(|source| limits.get(&source).copied());
            let settles = self.settles(spec, previous).await?;
            let waiting =
                due_at(spec, previous, streak, limited, settles).is_some_and(|due| due > now);
            let settled_before_boot = !self.ran_since_boot.lock().await.contains(spec.name)
                && previous.is_some_and(|run| {
                    matches!(run.outcome, Some(JobOutcome::Ok | JobOutcome::Noop))
                });
            let follows = schedule::FOLLOWERS.contains(&spec.name)
                && limited.is_none_or(|until| until <= now)
                && match previous {
                    Some(run) if matches!(run.outcome, Some(JobOutcome::Ok | JobOutcome::Noop)) => {
                        let pending = self.pending(spec).await?;
                        let seen = self.pending_seen.lock().await.get(spec.name).copied();
                        pending > 0 && seen.is_none_or(|seen| pending > seen)
                    }
                    _ => false,
                };
            if waiting && !follows && !(settled_before_boot && self.unread(spec).await?) {
                continue;
            }
            self.ran_since_boot.lock().await.insert(spec.name);
            let run_id = self.state.start_job(spec.name, now).await?;
            let job_started = tokio::time::Instant::now();
            let work = match self.run(spec.name, now).await {
                Ok(work) => work,
                Err(error) => Work::failed(error.to_string()),
            };
            let seconds = job_started.elapsed().as_secs_f64();
            if work.outcome == JobOutcome::Failed {
                tracing::error!(job = spec.name, seconds, message = %work.message, "ingest job failed");
            } else {
                tracing::info!(
                    job = spec.name,
                    outcome = ?work.outcome,
                    rows = work.rows,
                    seconds,
                    message = %work.message,
                    "ingest job finished"
                );
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
            drop(running);
            if work.rows > 0
                && let Err(error) = self.market.order_daily_bars().await
            {
                tracing::warn!(%error, "cannot put the daily bars back in date order");
            }
            report.ran.push((spec.name, work.outcome));
            let views = self.job_views(finished).await?;
            if let Some(job) = views.into_iter().find(|view| view.name == spec.name) {
                let _ = self.events.send(SpineEvent::Job { job });
            }
            if !spec.datasets.is_empty()
                && let Err(error) = self.refresh_contracts(finished).await
            {
                tracing::error!(%error, "cannot measure the coverage contracts");
            }
            if schedule::FOLLOWERS.contains(&spec.name) {
                let pending = self.pending(spec).await?;
                self.pending_seen.lock().await.insert(spec.name, pending);
            }
        }
        Ok(report)
    }

    async fn unread(&self, spec: &JobSpec) -> Result<bool, IngestError> {
        if self.series_unread(spec.name).await? {
            return Ok(true);
        }
        for contract in self.contracts().await? {
            if Some(contract.source) != spec.source || !spec.datasets.contains(&contract.dataset) {
                continue;
            }
            let peak = self
                .state
                .contract_peak(contract.dataset, contract.source)
                .await?;
            if peak.is_some_and(|peak| contract.done < peak) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn settles(
        &self,
        spec: &JobSpec,
        latest: Option<&JobRun>,
    ) -> Result<Option<Timestamp>, IngestError> {
        let Some(run) = latest.filter(|_| spec.name == schedule::BARS_1D) else {
            return Ok(None);
        };
        let from = run.started_at.exchange_date();
        let sessions = self
            .market
            .calendar(from, from + TimeDelta::days(SETTLE_LOOKAHEAD_DAYS))
            .await?;
        Ok(sessions
            .sessions()
            .iter()
            .map(|session| session.close + bars::settle_delay())
            .find(|settled| *settled > run.started_at))
    }

    async fn pending(&self, spec: &JobSpec) -> Result<u64, IngestError> {
        Ok(self
            .contracts()
            .await?
            .iter()
            .filter(|contract| spec.datasets.contains(&contract.dataset))
            .map(|contract| contract.pending)
            .sum())
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
            let first_due = self
                .state
                .first_job_start(spec.name)
                .await?
                .unwrap_or(self.booted_at);
            let settles = self.settles(&spec, previous).await?;
            let view = schedule::view(
                &spec,
                previous,
                due_at(&spec, previous, streak, limited, settles),
                success.as_ref(),
                first_due,
                now,
            );
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
        if tokio::time::timeout(grace, self.running.write())
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
            let foundation: Vec<&'static JobSpec> = JOBS
                .iter()
                .filter(|spec| schedule::FOUNDATION.contains(&spec.name))
                .collect();
            if let Err(error) = ingest.tick_lane(&foundation, Timestamp::now()).await {
                tracing::error!(%error, "ingest tick failed");
            }
            futures::future::join_all(schedule::lanes().into_iter().map(|lane| {
                let ingest = ingest.clone();
                async move {
                    let mut interval = tokio::time::interval(ingest.config.tick);
                    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    let follower = lane
                        .iter()
                        .any(|spec| schedule::FOLLOWERS.contains(&spec.name));
                    let mut measured = ingest.contracts.subscribe();
                    loop {
                        if follower {
                            tokio::select! {
                                _ = interval.tick() => {}
                                _ = measured.changed() => {}
                            }
                        } else {
                            interval.tick().await;
                        }
                        if let Err(error) = ingest.tick_lane(&lane, Timestamp::now()).await {
                            tracing::error!(%error, "ingest tick failed");
                        }
                    }
                }
            }))
            .await;
        }))
    }

    async fn run(&self, job: &str, now: Timestamp) -> Result<Work, IngestError> {
        match job {
            schedule::CALENDAR => self.calendar(now).await,
            schedule::INSTRUMENTS => self.instruments(now).await,
            schedule::BARS_1D => self.daily_bars(now).await,
            schedule::BARS_1M => self.minute_bars(now).await,
            schedule::CORPORATE_ACTIONS => self.corporate_actions(now).await,
            schedule::SEC_INDEX => self.sec_index(now).await,
            schedule::DELISTINGS => self.delistings(now).await,
            schedule::FUNDAMENTALS => self.fundamentals(now).await,
            schedule::FILINGS => self.filings(now).await,
            schedule::INSIDER_TRADES => self.insider_trades(now).await,
            schedule::PRESS_RELEASES => self.press_releases(now).await,
            schedule::FILING_DOCUMENTS => self.filing_documents(now).await,
            schedule::FILING_SECTIONS => self.filing_sections().await,
            schedule::RECIPE_FACTS => self.recipe_facts(now).await,
            schedule::NEWS => self.news(now).await,
            schedule::INDEX_MEMBERSHIP => self.index_membership(now).await,
            schedule::RATES => self.rates(now).await,
            schedule::BORROW => self.borrow(now).await,
            schedule::OPTION_CHAINS => self.option_chains(now).await,
            schedule::TRADING_HALTS => self.trading_halts(now).await,
            schedule::ETF_HOLDINGS => self.etf_holdings(now).await,
            schedule::NASDAQ_NORDIC => self.nasdaq_nordic(now).await,
            schedule::EURONEXT_OSLO => self.euronext_oslo(now).await,
            schedule::FX_RATES => self.fx_rates(now).await,
            schedule::SHORT_INTEREST => self.short_interest(now).await,
            schedule::SHORT_VOLUME => self.short_volume(now).await,
            schedule::FAILS_TO_DELIVER => self.fails_to_deliver(now).await,
            schedule::PAGE_VIEWS => self.page_views(now).await,
            schedule::MACRO => self.macro_series(now).await,
            schedule::VIX => self.vix(now).await,
            schedule::PUT_CALL => self.put_call(now).await,
            schedule::FOMC => self.fomc(now).await,
            schedule::EARNINGS_INSIGHT => self.earnings_insight(now).await,
            schedule::EARNINGS_INSIGHT_ARCHIVE => self.earnings_insight_archive(now).await,
            schedule::STATE_SNAPSHOT => self.state_snapshot(now).await,
            schedule::MARKET_SNAPSHOT => self.market_snapshot(now).await,
            schedule::TIINGO_HISTORY => self.tiingo_history(now).await,
            other => unreachable!("unscheduled job {other}"),
        }
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
            .filter(|instrument| {
                instrument.exchange != OTC && instrument.symbol.ended_listing().is_none()
            })
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
