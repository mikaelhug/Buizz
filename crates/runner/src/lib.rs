mod commit;
mod drive;
mod link;
mod report;
mod research;
mod timeline;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use axum::extract::ws::WebSocket;
use buizz_core::api::{
    ProcessSlots, RunSummary, SparkPoint, SpineEvent, StartBacktest, StartSweep, SweepStarted,
};
use buizz_core::market::{Instrument, StockIndex, Symbol, Timeframe};
use buizz_core::money::CASH_DECIMALS;
use buizz_core::ops::{CoverageGap, DataWarning};
use buizz_core::performance::Metrics;
use buizz_core::run::{
    DesiredState, MAX_SWEEP_VARIANTS, Mode, PeriodPreset, RunBadge, RunConfig, RunEvent,
    RunEventKind, RunId, RunStatus, UNIVERSE_SESSIONS, UniverseSpec, coerce_param,
};
use buizz_core::time::Timestamp;
use buizz_engine::ENGINE_VERSION;
use buizz_feed::{FeedError, FeedLease, FeedManager};
use buizz_ingest::{Ingest, IngestError};
use buizz_market::{MarketError, MarketStore};
use buizz_state::{NewRun, RunFilter, StateError, StateStore};
use buizz_supervisor::{BotScan, BotSpec, Supervisor, SupervisorError, load_bot, scan_bots};
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
}

impl Inner {
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
            let spec = match self.bot(&run.bot).await {
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
        let spec = self.bot(bot).await?;
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
        let config = self.run_config(
            &spec,
            Mode::Backtest,
            period.from,
            Some(period.to),
            holdout,
            request,
        )?;
        let warnings = self.covered(&config).await?;
        let mut runs = self
            .create(vec![config], &spec, RunStatus::Queued, warnings, None)
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
        let spec = self.bot(bot).await?;
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
                &spec,
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
        let already: Vec<Map<String, Value>> = self
            .inner
            .state
            .runs(RunFilter {
                bot: Some(spec.name.clone()),
                bot_hash: Some(spec.content_hash.clone()),
                mode: Some(Mode::Backtest),
                statuses: vec![
                    RunStatus::Queued,
                    RunStatus::Starting,
                    RunStatus::Running,
                    RunStatus::Completed,
                ],
                ..RunFilter::default()
            })
            .await?
            .into_iter()
            .filter(|run| {
                run.config.start == period.from
                    && run.config.end == Some(period.to)
                    && self.standard(&run.config, &spec)
            })
            .map(|run| run.config.params)
            .collect();
        let total = configs.len();
        configs.retain(|config| !already.contains(&config.params));
        let skipped = u32::try_from(total - configs.len()).unwrap_or(u32::MAX);
        let mut queued = Vec::new();
        if let Some(first) = configs.first() {
            let warnings = self.covered(first).await?;
            for run in self
                .create(configs, &spec, RunStatus::Queued, warnings, None)
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

    pub async fn cancel_queued(&self, bot: &str) -> Result<u32, RunnerError> {
        let queued = self
            .inner
            .state
            .runs(RunFilter {
                bot: Some(self.bot(bot).await?.name),
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
    ) -> Result<RunSummary, RunnerError> {
        let spec = self.bot(bot).await?;
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
                .is_none_or(|record| record.bot_hash != spec.content_hash)
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

    async fn bot(&self, name: &str) -> Result<BotSpec, RunnerError> {
        let (bots_dir, name) = (self.inner.config.bots_dir.clone(), name.to_owned());
        let loaded = tokio::task::spawn_blocking(move || load_bot(&bots_dir, &name))
            .await
            .expect("loading a bot does not panic");
        match loaded {
            Err(SupervisorError::NoSuchBot(name)) => Err(RunnerError::UnknownBot(name)),
            loaded => Ok(loaded?),
        }
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
            holdout,
        })
    }

    async fn covered(&self, config: &RunConfig) -> Result<Vec<DataWarning>, RunnerError> {
        let market = &self.inner.market;
        let symbols = gate_symbols(market, config).await?;
        let end = config.end.unwrap_or(config.start);
        let gaps = market
            .coverage_gaps(symbols.clone(), Timeframe::Day, config.start, end)
            .await?;
        if !gaps.is_empty() {
            return Err(RunnerError::CoverageGaps(gaps));
        }
        let mut warnings = market
            .unexplained_moves(symbols.clone(), config.start, end)
            .await?;
        warnings.extend(
            market
                .action_disagreements(Some(symbols), config.start, end)
                .await?,
        );
        Ok(warnings)
    }

    async fn create(
        &self,
        configs: Vec<RunConfig>,
        spec: &BotSpec,
        status: RunStatus,
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
        if let UniverseSpec::Index { index } = config.universe {
            let check = index_check(&inner.market, index, config).await?;
            minor_gaps = check.minor && warnings.is_empty();
            survivorship |= !check.unpriced.is_empty();
            if check.thin {
                badges.push(RunBadge::ThinIndexHistory);
            }
            warnings.extend(check.unpriced.into_iter().map(DataWarning::CoverageGap));
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
                engine_version: ENGINE_VERSION.to_string(),
                badges: badges.clone(),
                created_at,
                source_run,
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

fn describe(warning: &DataWarning) -> String {
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
        DataWarning::ActionDisagreement(disagreement) => {
            let value = |value: &Option<Decimal>| {
                value.map_or("none".to_owned(), |value| value.normalize().to_string())
            };
            format!(
                "Alpaca and Tiingo disagree on {}'s {:?} on {}: Alpaca {}, Tiingo {}; {:?} is used",
                disagreement.symbol,
                disagreement.action,
                disagreement.ex_date,
                value(&disagreement.alpaca),
                value(&disagreement.tiingo),
                disagreement.kept
            )
        }
    }
}
