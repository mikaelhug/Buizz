mod behaviour;
mod commit;
mod drive;
mod holdings;
mod link;
mod queue;
mod report;
mod research;
mod study;
mod timeline;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use axum::extract::ws::WebSocket;
use buizz_core::api::{
    ProcessSlots, RunSummary, ScreenedLive, ScreenedVariant, SparkPoint, SpineEvent, StartBacktest,
    StartSweep, SweepStarted,
};
use buizz_core::market::{Instrument, StockIndex, Symbol, Timeframe};
use buizz_core::money::CASH_DECIMALS;
use buizz_core::ops::{ActionVerdict, CoverageGap, DataWarning};
use buizz_core::performance::{Behaviour, Metrics};
use buizz_core::run::{
    DesiredState, LogLevel, MAX_LIVE_VARIANTS, MAX_SWEEP_VARIANTS, Mode, Period, PeriodPreset,
    RunBadge, RunConfig, RunEvent, RunEventKind, RunId, RunOrigin, RunStatus, UNIVERSE_SESSIONS,
    UniverseSpec, coerce_param,
};
use buizz_core::time::Timestamp;
use buizz_engine::ENGINE_VERSION;
use buizz_feed::{FeedError, FeedLease, FeedManager};
use buizz_ingest::{Ingest, IngestError};
use buizz_market::{MarketError, MarketStore};
use buizz_state::{NewRun, RunFilter, StateError, StateStore};
use buizz_supervisor::{
    BotScan, BotSpec, Supervisor, SupervisorError, UNMEASURED_RESERVATION, load_bot, scan_bots,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde_json::{Map, Value};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;

use crate::drive::Control;
pub use crate::research::Benchmarks;

const EVENT_BUFFER: usize = 4096;
const SEED_LIMIT: u64 = 1 << 53;
const SOCKET_QUEUE: usize = 4;

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
    #[error("bot {0} is already live with these params")]
    AlreadyLive(String),
    #[error("bot {bot} already has {limit} live variants; stop one first")]
    LiveVariantLimit { bot: String, limit: u32 },
    #[error("run {0} is not an active live run")]
    NotLive(RunId),
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
    pub auto_screen: bool,
}

pub struct RunnerDeps {
    pub market: MarketStore,
    pub state: StateStore,
    pub supervisor: Supervisor,
    pub feed: FeedManager,
    pub ingest: Ingest,
}

struct RunHandle {
    control: watch::Sender<Control>,
    sockets: mpsc::Sender<WebSocket>,
    task: JoinHandle<()>,
}

pub(crate) struct Inner {
    config: RunnerConfig,
    market: MarketStore,
    state: StateStore,
    supervisor: Supervisor,
    feed: FeedManager,
    ingest: Ingest,
    events: broadcast::Sender<SpineEvent>,
    tokens: Mutex<HashMap<String, RunId>>,
    runs: Mutex<HashMap<RunId, RunHandle>>,
    live_stats: tokio::sync::Mutex<()>,
    benchmarks: Mutex<Option<Arc<research::Benchmarks>>>,
    queue: queue::BacktestQueue,
    queueing: tokio::sync::Mutex<()>,
    unscreened: Mutex<BTreeMap<String, String>>,
    instruments: Arc<timeline::Instruments>,
}

impl Inner {
    async fn bot(&self, name: &str) -> Result<BotSpec, RunnerError> {
        let (bots_dir, name) = (self.config.bots_dir.clone(), name.to_owned());
        let loaded = tokio::task::spawn_blocking(move || load_bot(&bots_dir, &name))
            .await
            .expect("loading a bot does not panic");
        match loaded {
            Err(SupervisorError::NoSuchBot(name)) => Err(RunnerError::UnknownBot(name)),
            loaded => Ok(loaded?),
        }
    }

    async fn memory_need(&self, bot: &str, mode: Mode) -> Result<u64, StateError> {
        let footprints = self.state.footprints().await?;
        let peak = |wanted: Mode| {
            footprints
                .iter()
                .find(|footprint| footprint.bot == bot && footprint.mode == wanted)
                .map(|footprint| footprint.usage.peak_memory)
                .filter(|peak| *peak > 0)
        };
        let measured = match mode {
            Mode::Live => peak(Mode::Live).or_else(|| peak(Mode::Backtest)),
            Mode::Backtest => peak(Mode::Backtest),
        };
        Ok(measured.map_or(UNMEASURED_RESERVATION, |peak| peak * 2))
    }

    fn issue_token(&self, run_id: RunId) -> String {
        let token: String = rand::random::<[u8; 32]>()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.tokens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(token.clone(), run_id);
        token
    }

    fn forget(&self, run_id: RunId, token: &str) {
        self.tokens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(token);
        let mut runs = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
        if runs
            .get(&run_id)
            .is_some_and(|handle| handle.task.id() == tokio::task::id())
        {
            runs.remove(&run_id);
        }
    }

    fn broadcast(&self, event: SpineEvent) {
        let _ = self.events.send(event);
    }

    fn steer(&self, run_id: RunId, desired: DesiredState) -> bool {
        self.runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&run_id)
            .is_some_and(|handle| {
                handle.control.send_if_modified(|control| {
                    let changed = control.desired != desired;
                    control.desired = desired;
                    changed
                });
                true
            })
    }
}

#[derive(Clone)]
pub struct Runner {
    inner: Arc<Inner>,
}

impl Runner {
    pub fn new(config: RunnerConfig, deps: RunnerDeps) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                market: deps.market,
                state: deps.state,
                supervisor: deps.supervisor,
                feed: deps.feed,
                ingest: deps.ingest,
                events: broadcast::Sender::new(EVENT_BUFFER),
                tokens: Mutex::default(),
                runs: Mutex::default(),
                live_stats: tokio::sync::Mutex::default(),
                benchmarks: Mutex::default(),
                queue: queue::BacktestQueue::default(),
                queueing: tokio::sync::Mutex::default(),
                unscreened: Mutex::default(),
                instruments: Arc::default(),
            }),
        }
    }

    pub fn bots(&self) -> BotScan {
        scan_bots(&self.inner.config.bots_dir).unwrap_or_else(|error| BotScan {
            bots: Vec::new(),
            invalid: vec![error],
        })
    }

    pub async fn recover(&self) -> Result<(), RunnerError> {
        let state = &self.inner.state;
        let mut runs = state
            .runs(RunFilter {
                statuses: RunStatus::ACTIVE.to_vec(),
                ..RunFilter::default()
            })
            .await?;
        runs.reverse();
        for mut run in runs {
            if run.desired == DesiredState::Stopped {
                self.conclude(run.id, RunStatus::Cancelled, None).await?;
                continue;
            }
            let spec = match self.inner.bot(&run.bot).await {
                Ok(spec) => spec,
                Err(error) => {
                    self.conclude(run.id, RunStatus::Failed, Some(error.to_string()))
                        .await?;
                    continue;
                }
            };
            if run.mode == Mode::Backtest {
                state
                    .set_status(run.id, RunStatus::Queued, None, Timestamp::now())
                    .await?;
                run = self.summary(run.id).await?;
            }
            self.spawn(run, spec, None);
        }
        Ok(())
    }

    pub async fn start_backtest(
        &self,
        bot: &str,
        request: StartBacktest,
        today: NaiveDate,
    ) -> Result<RunSummary, RunnerError> {
        let spec = self.inner.bot(bot).await?;
        let config = self.backtest_config(&spec, request, today)?;
        let warnings = self.covered(&config).await?;
        let mut runs = self
            .create(
                vec![config],
                &spec,
                RunStatus::Queued,
                RunOrigin::Request,
                warnings,
                None,
            )
            .await?;
        let run = runs.remove(0);
        self.spawn(run.clone(), spec, None);
        Ok(run)
    }

    pub async fn sweep(
        &self,
        bot: &str,
        request: StartSweep,
        today: NaiveDate,
    ) -> Result<SweepStarted, RunnerError> {
        let spec = self.inner.bot(bot).await?;
        self.sweep_as(&spec, request, today, RunOrigin::Request)
            .await
    }

    async fn sweep_as(
        &self,
        spec: &BotSpec,
        request: StartSweep,
        today: NaiveDate,
        origin: RunOrigin,
    ) -> Result<SweepStarted, RunnerError> {
        if request.preset == PeriodPreset::Holdout {
            return Err(RunnerError::InvalidRequest(
                "sweeps never run on the locked holdout".to_owned(),
            ));
        }
        let period = request.preset.resolve(today);
        let seed = rand::random_range(0..SEED_LIMIT);
        let mut configs: Vec<RunConfig> = Vec::new();
        for params in grid_variants(request.grid)? {
            let config = self.run_config(
                spec,
                Mode::Backtest,
                period.from,
                Some(period.to),
                false,
                StartBacktest {
                    preset: Some(request.preset),
                    params: Some(params),
                    seed: Some(seed),
                    ..StartBacktest::default()
                },
            )?;
            if !configs.iter().any(|seen| seen.params == config.params) {
                configs.push(config);
            }
        }
        let _queueing = self.inner.queueing.lock().await;
        let tried = self.tried(spec).await?;
        let total = configs.len();
        configs.retain(|config| !tried.iter().any(|run| same_trial(&run.config, config)));
        let skipped = u32::try_from(total - configs.len()).unwrap_or(u32::MAX);
        let mut queued = Vec::new();
        if let Some(first) = configs.first() {
            let warnings = self.covered(first).await?;
            for run in self
                .create(configs, spec, RunStatus::Queued, origin, warnings, None)
                .await?
            {
                queued.push(run.id);
                self.spawn(run, spec.clone(), None);
            }
        }
        Ok(SweepStarted {
            period,
            queued,
            skipped,
            trials: self.inner.state.trials(&spec.name).await?,
        })
    }

    pub async fn screen(&self, today: NaiveDate) {
        if !self.inner.config.auto_screen {
            return;
        }
        let runner = self.clone();
        let scan = tokio::task::spawn_blocking(move || runner.bots())
            .await
            .expect("scanning the bots does not panic");
        let paces = self
            .inner
            .state
            .backtest_paces()
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "cannot read the bots' backtest paces");
                BTreeMap::new()
            });
        let typical = median(paces.values().copied());
        let pace = |spec: &BotSpec| paces.get(&spec.name).copied().unwrap_or(typical);
        let mut bots: Vec<BotSpec> = scan
            .bots
            .into_iter()
            .filter(|spec| spec.manifest.auto_screen)
            .collect();
        bots.sort_by(|left, right| pace(left).total_cmp(&pace(right)));
        for spec in bots {
            let reason = self.screen_bot(&spec, today).await.err();
            if let Some(reason) = &reason {
                tracing::info!(bot = %spec.name, %reason, "not screened");
            }
            let mut unscreened = self
                .inner
                .unscreened
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let before = match &reason {
                Some(reason) => unscreened.insert(spec.name.clone(), reason.clone()),
                None => unscreened.remove(&spec.name),
            };
            drop(unscreened);
            if before != reason {
                self.inner.broadcast(SpineEvent::Unscreened {
                    bot: spec.name,
                    reason,
                });
            }
        }
    }

    pub fn unscreened(&self, bot: &str) -> Option<String> {
        self.inner
            .unscreened
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(bot)
            .cloned()
    }

    async fn screen_bot(&self, spec: &BotSpec, today: NaiveDate) -> Result<(), String> {
        let preset = PeriodPreset::TenYears;
        let config = self
            .backtest_config(
                spec,
                StartBacktest {
                    preset: Some(preset),
                    ..StartBacktest::default()
                },
                today,
            )
            .map_err(|error| error.to_string())?;
        let end = config.end.unwrap_or(config.start);
        let daily = self.inner.market.watermarks().borrow().get(Timeframe::Day);
        if daily.is_none_or(|through| through.exchange_date() < end) {
            return Err(format!(
                "the store's daily bars do not reach {end}, the end of the 10y preset, yet"
            ));
        }
        if config.needs_minute_bars() {
            let market = &self.inner.market;
            let symbols = gate_symbols(market, &config)
                .await
                .map_err(|error| error.to_string())?;
            let gaps = market
                .coverage_gaps(symbols, Timeframe::Minute, config.start, end)
                .await
                .map_err(|error| error.to_string())?;
            if let Some(gap) = gaps.first() {
                return Err(format!(
                    "the store lacks the 1m bars it needs on the 10y preset ({} gaps, the first \
                     {} from {} to {}); a backtest started by hand fetches them",
                    gaps.len(),
                    gap.symbol,
                    gap.from,
                    gap.to
                ));
            }
        }
        let request = StartSweep {
            preset,
            grid: BTreeMap::new(),
        };
        match self
            .sweep_as(spec, request, today, RunOrigin::Screening)
            .await
        {
            Ok(_) => Ok(()),
            Err(RunnerError::CoverageGaps(gaps)) => Err(match gaps.first() {
                Some(gap) => format!(
                    "market data has {} gaps on the 10y preset, the first {} from {} to {}",
                    gaps.len(),
                    gap.symbol,
                    gap.from,
                    gap.to
                ),
                None => "market data has gaps on the 10y preset".to_owned(),
            }),
            Err(error) => Err(error.to_string()),
        }
    }

    pub(crate) async fn requeue(
        &self,
        run_id: RunId,
        why: String,
        started: bool,
    ) -> Result<(), RunnerError> {
        let _queueing = self.inner.queueing.lock().await;
        let run = self.summary(run_id).await?;
        let replaced = self.replacement(&run, &why).await;
        let message = match &replaced {
            Ok(Replacement::Queued(queued)) => {
                format!(
                    "{why}; queued again on the current code as run {}",
                    queued.0.id
                )
            }
            Ok(Replacement::Running(existing)) => {
                format!("{why}; run {existing} already runs it on the current code")
            }
            Err(error) => format!("{why}; it cannot be queued again: {error}"),
        };
        let status = if started {
            RunStatus::Failed
        } else {
            RunStatus::Cancelled
        };
        self.conclude(run_id, status, Some(message)).await?;
        if let Ok(Replacement::Queued(queued)) = replaced {
            let (new, spec) = *queued;
            self.spawn(new, spec, None);
        }
        Ok(())
    }

    async fn replacement(&self, run: &RunSummary, why: &str) -> Result<Replacement, RunnerError> {
        let spec = self.inner.bot(&run.bot).await?;
        let old = &run.config;
        let period = Period {
            from: old.start,
            to: old.end.unwrap_or(old.start),
        };
        let preset = PeriodPreset::of(period, run.created_at.exchange_date());
        let request = StartBacktest {
            preset,
            period: preset.is_none().then_some(period),
            params: Some(
                old.params
                    .iter()
                    .filter(|(name, _)| spec.manifest.params.contains_key(*name))
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
            ),
            start_cash: Some(old.start_cash),
            cost: Some(old.cost.clone()),
            risk: Some(old.risk),
            seed: Some(old.seed),
            allow_holdout: old.holdout,
        };
        let config = self.backtest_config(&spec, request, Timestamp::now().exchange_date())?;
        if let Some(existing) = self
            .tried(&spec)
            .await?
            .into_iter()
            .find(|tried| same_trial(&tried.config, &config))
        {
            return Ok(Replacement::Running(existing.id));
        }
        let warnings = self.covered(&config).await?;
        let new = self
            .create(
                vec![config],
                &spec,
                RunStatus::Queued,
                run.origin,
                warnings,
                None,
            )
            .await?
            .remove(0);
        let seq = drive::last_event_seq(&self.inner, new.id).await? + 1;
        self.inner
            .state
            .append_events(
                new.id,
                vec![RunEvent {
                    seq,
                    at: Timestamp::now(),
                    clock: None,
                    kind: RunEventKind::Log {
                        level: LogLevel::Info,
                        message: format!("queued again from run {}: {why}", run.id),
                    },
                }],
            )
            .await?;
        Ok(Replacement::Queued(Box::new((new, spec))))
    }

    async fn tried(&self, spec: &BotSpec) -> Result<Vec<RunSummary>, RunnerError> {
        Ok(self
            .inner
            .state
            .runs(RunFilter {
                bot: Some(spec.name.clone()),
                code: Some(spec.code()),
                mode: Some(Mode::Backtest),
                statuses: vec![
                    RunStatus::Queued,
                    RunStatus::Starting,
                    RunStatus::Running,
                    RunStatus::Completed,
                    RunStatus::Failed,
                ],
                ..RunFilter::default()
            })
            .await?
            .into_iter()
            .filter(|run| {
                run.status != RunStatus::Failed || run.badges.contains(&RunBadge::DrawdownStop)
            })
            .collect())
    }

    pub async fn cancel_queued(&self, bot: &str) -> Result<u32, RunnerError> {
        let queued = self
            .inner
            .state
            .runs(RunFilter {
                bot: Some(self.inner.bot(bot).await?.name),
                mode: Some(Mode::Backtest),
                statuses: vec![RunStatus::Queued],
                ..RunFilter::default()
            })
            .await?;
        for run in &queued {
            self.cancel(run.id, None).await?;
        }
        Ok(u32::try_from(queued.len()).unwrap_or(u32::MAX))
    }

    pub async fn go_live(
        &self,
        bot: &str,
        from_run: Option<RunId>,
        origin: RunOrigin,
    ) -> Result<RunSummary, RunnerError> {
        let spec = self.inner.bot(bot).await?;
        let today = Timestamp::now().exchange_date();
        let mut request = StartBacktest::default();
        if let Some(source) = from_run {
            let run = self.summary(source).await?;
            if run.bot != spec.name || run.mode != Mode::Backtest {
                return Err(RunnerError::InvalidRequest(format!(
                    "run {source} is not a backtest of {}",
                    spec.name
                )));
            }
            if run
                .reproducibility
                .is_none_or(|record| !record.code().same_as(&spec.code()))
            {
                return Err(RunnerError::InvalidRequest(format!(
                    "run {source} backtested older code of {}; backtest the current code first",
                    spec.name
                )));
            }
            request.params = Some(run.config.params);
            request.risk = Some(run.config.risk);
        }
        let config = self.run_config(&spec, Mode::Live, today, None, false, request)?;
        let streamed = if config.timeframe == Timeframe::Minute {
            let universe = universe_at(&self.inner.market, &config, today).await?;
            self.inner.feed.admits(universe.clone())?;
            Some(universe)
        } else {
            if config.needs_minute_bars() {
                self.inner.feed.fetches_minutes()?;
            }
            None
        };
        let run = match self
            .create(
                vec![config],
                &spec,
                RunStatus::Starting,
                origin,
                Vec::new(),
                from_run,
            )
            .await
        {
            Err(RunnerError::State(StateError::LiveRunExists(bot))) => {
                return Err(RunnerError::AlreadyLive(bot));
            }
            Err(RunnerError::State(StateError::LiveVariantLimit { bot, limit })) => {
                return Err(RunnerError::LiveVariantLimit { bot, limit });
            }
            created => created?.remove(0),
        };
        let lease =
            streamed.and_then(|universe| self.inner.feed.acquire(run.id, &run.bot, universe).ok());
        self.spawn(run.clone(), spec, lease);
        Ok(run)
    }

    pub async fn go_live_screened(&self) -> Result<ScreenedLive, RunnerError> {
        let benchmarks = self.benchmarks();
        let live = self
            .inner
            .state
            .runs(RunFilter {
                mode: Some(Mode::Live),
                statuses: RunStatus::ACTIVE.to_vec(),
                ..RunFilter::default()
            })
            .await?;
        let mut outcome = ScreenedLive::default();
        for spec in self.bots().bots {
            let mut variants: Vec<&Map<String, Value>> = live
                .iter()
                .filter(|run| run.bot == spec.name)
                .map(|run| &run.config.params)
                .collect();
            let passing = self.passing_variants(&spec, &benchmarks).await?;
            if passing.is_empty() {
                outcome.skipped.push(ScreenedVariant {
                    bot: spec.name.clone(),
                    from_run: None,
                    params: None,
                    run_id: None,
                    reason: Some("no variant passes the screening gate".to_owned()),
                });
            }
            for source in &passing {
                let skip = if variants.contains(&&source.config.params) {
                    Some("already live".to_owned())
                } else if variants.len() >= MAX_LIVE_VARIANTS as usize {
                    Some(format!("the bot has {MAX_LIVE_VARIANTS} live variants"))
                } else {
                    None
                };
                let started = match skip {
                    Some(reason) => Err(reason),
                    None => self
                        .go_live(&spec.name, Some(source.id), RunOrigin::Screening)
                        .await
                        .map_err(|error| error.to_string()),
                };
                let variant = ScreenedVariant {
                    bot: spec.name.clone(),
                    from_run: Some(source.id),
                    params: Some(source.config.params.clone()),
                    run_id: started.as_ref().ok().map(|run| run.id),
                    reason: started.as_ref().err().cloned(),
                };
                match started {
                    Ok(_) => {
                        variants.push(&source.config.params);
                        outcome.started.push(variant);
                    }
                    Err(_) => outcome.skipped.push(variant),
                }
            }
        }
        Ok(outcome)
    }

    pub async fn pause(&self, run_id: RunId) -> Result<RunSummary, RunnerError> {
        let run = self.active_live(run_id).await?;
        if run.desired == DesiredState::Running {
            self.inner
                .state
                .set_desired(run.id, DesiredState::Paused)
                .await?;
            self.inner.steer(run.id, DesiredState::Paused);
        }
        self.summary(run.id).await
    }

    pub async fn resume(&self, run_id: RunId) -> Result<RunSummary, RunnerError> {
        let run = self.active_live(run_id).await?;
        if run.desired == DesiredState::Paused {
            if run.config.timeframe == Timeframe::Minute {
                let today = Timestamp::now().exchange_date();
                let portfolio = self.inner.state.portfolio(run.id).await?;
                let held = portfolio.open_positions().map(|(symbol, _)| symbol.clone());
                let universe = universe_at(&self.inner.market, &run.config, today).await?;
                self.inner.feed.admits(universe.into_iter().chain(held))?;
            } else if run.config.needs_minute_bars() {
                self.inner.feed.fetches_minutes()?;
            }
            self.inner
                .state
                .set_desired(run.id, DesiredState::Running)
                .await?;
            self.inner.steer(run.id, DesiredState::Running);
        }
        self.summary(run.id).await
    }

    pub async fn cancel(
        &self,
        run_id: RunId,
        reason: Option<String>,
    ) -> Result<RunSummary, RunnerError> {
        let run = self.summary(run_id).await?;
        if run.status.is_finished() {
            return Ok(run);
        }
        self.inner.state.stop(run_id, reason).await?;
        let signalled = self.inner.steer(run_id, DesiredState::Stopped);
        if !signalled {
            self.conclude(run_id, RunStatus::Cancelled, None).await?;
        }
        self.summary(run_id).await
    }

    pub fn authorize(&self, token: &str) -> Option<RunId> {
        self.inner
            .tokens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(token)
            .copied()
    }

    pub async fn attach_bot(&self, run_id: RunId, socket: WebSocket) {
        let sockets = self
            .inner
            .runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&run_id)
            .map(|handle| handle.sockets.clone());
        if let Some(sockets) = sockets {
            let _ = sockets.send(socket).await;
        }
    }

    pub fn events(&self) -> broadcast::Receiver<SpineEvent> {
        self.inner.events.subscribe()
    }

    pub fn log_path(&self, run_id: RunId) -> PathBuf {
        self.inner.supervisor.log_path(run_id)
    }

    pub fn process_slots(&self) -> ProcessSlots {
        self.inner.supervisor.slots()
    }

    pub async fn benchmark(&self, run: &RunSummary) -> Result<Vec<SparkPoint>, RunnerError> {
        report::benchmark(&self.inner.market, run, Timestamp::now().exchange_date()).await
    }

    pub async fn metrics(&self, run: &RunSummary) -> Result<Metrics, RunnerError> {
        report::metrics(&self.inner.market, &self.inner.state, run).await
    }

    pub async fn behaviour(&self, run: &RunSummary) -> Result<Behaviour, RunnerError> {
        behaviour::behaviour(
            &self.inner.market,
            &self.inner.state,
            run,
            Timestamp::now().exchange_date(),
        )
        .await
    }

    pub async fn benchmark_metrics(
        &self,
        run: &RunSummary,
        benchmark: &[SparkPoint],
    ) -> Result<Metrics, RunnerError> {
        Ok(
            report::benchmark_metrics(&self.inner.market, run, benchmark)
                .await?
                .0,
        )
    }

    pub async fn shutdown(&self) {
        let handles: Vec<RunHandle> = self
            .inner
            .runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain()
            .map(|(_, handle)| handle)
            .collect();
        for handle in &handles {
            handle
                .control
                .send_modify(|control| control.shutdown = true);
        }
        for handle in handles {
            let _ = handle.task.await;
        }
    }

    fn backtest_config(
        &self,
        spec: &BotSpec,
        request: StartBacktest,
        today: NaiveDate,
    ) -> Result<RunConfig, RunnerError> {
        let period = match (request.period, request.preset) {
            (Some(period), _) => period,
            (None, Some(preset)) => preset.resolve(today),
            (None, None) => {
                return Err(RunnerError::InvalidRequest(
                    "choose a period or a preset".to_owned(),
                ));
            }
        };
        if period.from > period.to {
            return Err(RunnerError::InvalidRequest(format!(
                "the period starts on {} after it ends on {}",
                period.from, period.to
            )));
        }
        let holdout = period.touches_holdout(today);
        let explicit = request.allow_holdout || request.preset == Some(PeriodPreset::Holdout);
        if holdout && !explicit {
            return Err(RunnerError::HoldoutLocked);
        }
        self.run_config(
            spec,
            Mode::Backtest,
            period.from,
            Some(period.to),
            holdout,
            request,
        )
    }

    fn run_config(
        &self,
        spec: &BotSpec,
        mode: Mode,
        start: NaiveDate,
        end: Option<NaiveDate>,
        holdout: bool,
        request: StartBacktest,
    ) -> Result<RunConfig, RunnerError> {
        let defaults = &self.inner.config;
        let manifest = &spec.manifest;
        let mut params = manifest.params.clone();
        let requested = request.params.unwrap_or_default();
        let unknown: Vec<&str> = requested
            .keys()
            .filter(|name| !params.contains_key(*name))
            .map(String::as_str)
            .collect();
        if !unknown.is_empty() {
            let known: Vec<&str> = params.keys().map(String::as_str).collect();
            return Err(RunnerError::InvalidRequest(format!(
                "bot {} has no params {}; its params are {}",
                spec.name,
                unknown.join(", "),
                if known.is_empty() {
                    "none".to_owned()
                } else {
                    known.join(", ")
                }
            )));
        }
        for (name, value) in requested {
            let coerced = coerce_param(&params[&name], value).map_err(|problem| {
                RunnerError::InvalidRequest(format!("param {name} {problem}"))
            })?;
            params.insert(name, coerced);
        }
        let start_cash = request.start_cash.unwrap_or(defaults.start_cash);
        if start_cash <= Decimal::ZERO || start_cash.normalize().scale() > CASH_DECIMALS {
            return Err(RunnerError::InvalidRequest(
                "start cash must be positive with at most 2 decimals".to_owned(),
            ));
        }
        let seed = match request.seed {
            Some(seed) if seed >= SEED_LIMIT => {
                return Err(RunnerError::InvalidRequest(format!(
                    "the seed must be below {SEED_LIMIT}"
                )));
            }
            Some(seed) => seed,
            None => rand::random_range(0..SEED_LIMIT),
        };
        let cost = request.cost.unwrap_or_default();
        if let Some(problem) = cost.invalid() {
            return Err(RunnerError::InvalidRequest(problem.to_owned()));
        }
        let risk = request.risk.unwrap_or(manifest.risk);
        if !risk.are_positive() {
            return Err(RunnerError::InvalidRequest(
                "risk limits must be positive".to_owned(),
            ));
        }
        Ok(RunConfig {
            bot: spec.name.clone(),
            mode,
            params,
            universe: manifest.universe.clone(),
            timeframe: manifest.timeframe,
            wake: manifest.wake,
            start,
            end,
            start_cash,
            cost,
            risk,
            seed,
            benchmark: manifest.benchmark.clone(),
            hedge_benchmark: manifest.hedge_benchmark.clone(),
            holdout,
        })
    }

    async fn covered(&self, config: &RunConfig) -> Result<Vec<DataWarning>, RunnerError> {
        let market = &self.inner.market;
        let end = config.end.unwrap_or(config.start);
        let required = match config.universe {
            UniverseSpec::Index { .. } => vec![config.benchmark.clone()],
            _ => gate_symbols(market, config).await?,
        };
        let gaps = market
            .coverage_gaps(required.clone(), Timeframe::Day, config.start, end)
            .await?;
        if !gaps.is_empty() {
            return Err(RunnerError::CoverageGaps(gaps));
        }
        let mut warnings = market
            .unexplained_moves(required.clone(), config.start, end)
            .await?;
        warnings.extend(
            market
                .action_disagreements(Some(required), config.start, end)
                .await?,
        );
        if matches!(config.universe, UniverseSpec::TopDollarVolume { .. }) {
            warnings.extend(market.survivorship_gaps(config.start, end).await?);
        }
        Ok(warnings)
    }

    async fn create(
        &self,
        configs: Vec<RunConfig>,
        spec: &BotSpec,
        status: RunStatus,
        origin: RunOrigin,
        mut warnings: Vec<DataWarning>,
        source_run: Option<RunId>,
    ) -> Result<Vec<RunSummary>, RunnerError> {
        let inner = &self.inner;
        let Some(config) = configs.first() else {
            return Ok(Vec::new());
        };
        let instruments = inner.market.instruments().await?;
        let mut badges = Vec::new();
        let mut survivorship = survivorship_biased(&config.universe, &instruments);
        let mut minor_gaps = false;
        let mut exposures = Vec::new();
        if let UniverseSpec::Index { index } = config.universe {
            let check = index_check(&inner.market, index, config).await?;
            minor_gaps = check.minor && warnings.is_empty();
            survivorship |= !check.unpriced.is_empty();
            if check.thin {
                badges.push(RunBadge::ThinIndexHistory);
            }
            warnings.extend(check.unpriced.into_iter().map(DataWarning::CoverageGap));
            let end = config.end.unwrap_or(config.start);
            exposures = inner
                .market
                .member_warnings(index, config.start, end)
                .await?;
        }
        if survivorship {
            badges.push(RunBadge::SurvivorshipBiased);
        }
        if config.holdout {
            badges.push(RunBadge::Holdout);
        }
        if minor_gaps {
            badges.push(RunBadge::MinorDataGaps);
        } else if !warnings.is_empty() {
            badges.push(RunBadge::DataWarnings);
        }
        let created_at = Timestamp::now();
        let data_watermark = inner.market.watermarks().borrow().get(config.timeframe);
        let events: Vec<RunEvent> = warnings
            .iter()
            .chain(&exposures)
            .zip(1..)
            .map(|(warning, seq)| RunEvent {
                seq,
                at: created_at,
                clock: None,
                kind: RunEventKind::DataWarning {
                    message: describe(warning),
                },
            })
            .collect();
        let runs = configs
            .into_iter()
            .map(|config| NewRun {
                data_watermark,
                config,
                status,
                desired: DesiredState::Running,
                bot_hash: spec.content_hash.clone(),
                behaviour_hash: spec.behaviour_hash.clone(),
                engine_version: ENGINE_VERSION.to_string(),
                badges: badges.clone(),
                created_at,
                source_run,
                origin,
                events: events.clone(),
            })
            .collect();
        let mut created = Vec::new();
        for run_id in inner.state.create_runs(runs).await? {
            let run = self.summary(run_id).await?;
            inner.broadcast(SpineEvent::RunStatus {
                run_id,
                bot: run.bot.clone(),
                mode: run.mode,
                status: run.status,
            });
            created.push(run);
        }
        Ok(created)
    }

    async fn summary(&self, run_id: RunId) -> Result<RunSummary, RunnerError> {
        self.inner
            .state
            .run(run_id)
            .await?
            .ok_or(RunnerError::UnknownRun(run_id))
    }

    async fn active_live(&self, run_id: RunId) -> Result<RunSummary, RunnerError> {
        let run = self.summary(run_id).await?;
        if run.mode != Mode::Live
            || run.status.is_finished()
            || run.desired == DesiredState::Stopped
        {
            return Err(RunnerError::NotLive(run_id));
        }
        Ok(run)
    }

    async fn conclude(
        &self,
        run_id: RunId,
        status: RunStatus,
        message: Option<String>,
    ) -> Result<(), RunnerError> {
        drive::conclude(&self.inner, run_id, status, message, None, None).await?;
        Ok(())
    }

    fn spawn(&self, run: RunSummary, spec: BotSpec, lease: Option<FeedLease>) {
        let (control, receiver) = watch::channel(Control {
            desired: run.desired,
            shutdown: false,
        });
        let (sockets, incoming) = mpsc::channel(SOCKET_QUEUE);
        let run_id = run.id;
        let mut runs = self
            .inner
            .runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let task = tokio::spawn(drive::run(drive::Launch {
            inner: Arc::clone(&self.inner),
            run,
            spec,
            control: control.clone(),
            receiver,
            sockets: incoming,
            lease,
        }));
        runs.insert(
            run_id,
            RunHandle {
                control,
                sockets,
                task,
            },
        );
    }
}

async fn universe_at(
    market: &MarketStore,
    config: &RunConfig,
    date: NaiveDate,
) -> Result<Vec<Symbol>, MarketError> {
    market
        .universe(&config.universe, date, UNIVERSE_SESSIONS)
        .await
}

pub(crate) async fn gate_symbols(
    market: &MarketStore,
    config: &RunConfig,
) -> Result<Vec<Symbol>, MarketError> {
    let mut symbols = universe_at(market, config, config.start).await?;
    symbols.push(config.benchmark.clone());
    symbols.sort();
    symbols.dedup();
    Ok(symbols)
}

enum Replacement {
    Queued(Box<(RunSummary, BotSpec)>),
    Running(RunId),
}

fn same_trial(left: &RunConfig, right: &RunConfig) -> bool {
    RunConfig {
        seed: right.seed,
        ..left.clone()
    } == *right
}

fn median(values: impl Iterator<Item = f64>) -> f64 {
    let mut values: Vec<f64> = values.collect();
    values.sort_by(f64::total_cmp);
    values.get(values.len() / 2).copied().unwrap_or(0.0)
}

fn grid_variants(
    grid: BTreeMap<String, Vec<Value>>,
) -> Result<Vec<Map<String, Value>>, RunnerError> {
    let mut count: usize = 1;
    for (name, values) in &grid {
        if values.is_empty() {
            return Err(RunnerError::InvalidRequest(format!(
                "param {name} has no values to sweep"
            )));
        }
        count = count.saturating_mul(values.len());
    }
    if count > MAX_SWEEP_VARIANTS {
        return Err(RunnerError::InvalidRequest(format!(
            "the grid makes {count} variants; a sweep runs at most {MAX_SWEEP_VARIANTS}"
        )));
    }
    Ok(grid
        .into_iter()
        .fold(vec![Map::new()], |variants, (name, values)| {
            variants
                .iter()
                .flat_map(|variant| {
                    values.iter().map(|value| {
                        let mut next = variant.clone();
                        next.insert(name.clone(), value.clone());
                        next
                    })
                })
                .collect()
        }))
}

fn survivorship_biased(universe: &UniverseSpec, instruments: &[Instrument]) -> bool {
    match universe {
        UniverseSpec::Symbols { .. } => true,
        UniverseSpec::TopDollarVolume { .. } => !instruments
            .iter()
            .any(|instrument| !instrument.active || instrument.last_session.is_some()),
        UniverseSpec::Index { .. } => false,
    }
}

struct IndexCheck {
    thin: bool,
    unpriced: Vec<CoverageGap>,
    minor: bool,
}

const MINOR_GAPS_PER_MEMBER_SESSION: u64 = 200;

async fn index_check(
    market: &MarketStore,
    index: StockIndex,
    config: &RunConfig,
) -> Result<IndexCheck, RunnerError> {
    let history = market.index_history(index).await?.ok_or_else(|| {
        RunnerError::InvalidRequest(
            "no index membership is stored yet; the index_membership ingest job fills it"
                .to_owned(),
        )
    })?;
    let end = config.end.unwrap_or(config.start);
    let unpriced = market.unpriced_members(index, config.start, end).await?;
    let calendar = market.calendar(config.start, end).await?;
    let sessions = |from: NaiveDate, to: NaiveDate| calendar.between(from, to).len() as u64;
    let member_sessions: u64 = history
        .members
        .iter()
        .map(|member| {
            let from = member
                .start
                .map_or(config.start, |start| start.max(config.start));
            let to = member
                .end
                .and_then(|left| left.pred_opt())
                .map_or(end, |last| last.min(end));
            sessions(from, to)
        })
        .sum();
    let missing: u64 = unpriced.iter().map(|gap| sessions(gap.from, gap.to)).sum();
    Ok(IndexCheck {
        thin: history
            .reliable_from
            .is_none_or(|reliable| config.start < reliable),
        minor: !unpriced.is_empty() && missing * MINOR_GAPS_PER_MEMBER_SESSION <= member_sessions,
        unpriced,
    })
}

pub(crate) fn describe(warning: &DataWarning) -> String {
    match warning {
        DataWarning::UnexplainedMove {
            symbol,
            date,
            change,
        } => format!(
            "{symbol} opened {:+.1}% on {date} with no corporate action",
            change * 100.0
        ),
        DataWarning::CoverageGap(gap) => format!(
            "{} has no {} bars from {} to {}",
            gap.symbol, gap.timeframe, gap.from, gap.to
        ),
        DataWarning::OverlapRejected {
            symbol,
            source,
            message,
        } => format!("{symbol} from {source:?} was rejected: {message}"),
        DataWarning::SurvivorshipGap {
            from,
            to,
            delistings: 0,
            ..
        } => format!(
            "before Alpaca's history the universe from {from} to {to} holds only companies still trading in 2016"
        ),
        DataWarning::SurvivorshipGap {
            from,
            to,
            delistings,
            unpriced,
        } => format!(
            "{unpriced} of {delistings} stock delistings from {from} to {to} have no prices, so the universe misses those losers"
        ),
        DataWarning::ActionDisagreement(disagreement) => {
            let value = |value: &Option<Decimal>| {
                value.map_or("none".to_owned(), |value| value.normalize().to_string())
            };
            let used = match disagreement.kept {
                ActionVerdict::Alpaca => "Alpaca's is used",
                ActionVerdict::Tiingo => "Tiingo's is used",
                ActionVerdict::Prices => "the prices refute Alpaca's, so none is applied",
            };
            format!(
                "Alpaca and Tiingo disagree on {}'s {:?} on {}: Alpaca {}, Tiingo {}; {used}",
                disagreement.symbol,
                disagreement.action,
                disagreement.ex_date,
                value(&disagreement.alpaca),
                value(&disagreement.tiingo),
            )
        }
    }
}
