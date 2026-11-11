mod commit;
mod drive;
mod link;
mod report;
mod timeline;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use axum::extract::ws::WebSocket;
use buizz_core::api::{RunSummary, SparkPoint, SpineEvent, StartBacktest};
use buizz_core::market::{Instrument, Symbol, Timeframe};
use buizz_core::ops::{CoverageGap, DataWarning};
use buizz_core::run::{
    CostModel, DesiredState, Mode, PeriodPreset, RiskLimits, RunBadge, RunConfig, RunEvent,
    RunEventKind, RunId, RunStatus, UniverseSpec,
};
use buizz_core::time::Timestamp;
use buizz_engine::ENGINE_VERSION;
use buizz_feed::{FeedError, FeedLease, FeedManager};
use buizz_ingest::{Ingest, IngestError};
use buizz_market::{MarketError, MarketStore};
use buizz_state::{NewRun, RunFilter, StateError, StateStore};
use buizz_supervisor::{BotScan, BotSpec, Supervisor, SupervisorError, scan_bots};
use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;

use crate::drive::Control;

const EVENT_BUFFER: usize = 4096;
const SEED_LIMIT: u64 = 1 << 53;
const SOCKET_QUEUE: usize = 4;
const ACTIVE: [RunStatus; 4] = [
    RunStatus::Queued,
    RunStatus::Starting,
    RunStatus::Running,
    RunStatus::Paused,
];

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
                statuses: ACTIVE.to_vec(),
                ..RunFilter::default()
            })
            .await?;
        runs.reverse();
        let scan = self.bots();
        for mut run in runs {
            if run.desired == DesiredState::Stopped {
                self.conclude(run.id, RunStatus::Cancelled, None).await?;
                continue;
            }
            let Some(spec) = scan.bots.iter().find(|spec| spec.name == run.bot).cloned() else {
                let message = format!("bot folder {} is gone", run.bot);
                self.conclude(run.id, RunStatus::Failed, Some(message))
                    .await?;
                continue;
            };
            let mut lease = None;
            if run.mode == Mode::Backtest {
                state
                    .set_status(run.id, RunStatus::Queued, None, now())
                    .await?;
            } else if run.config.timeframe == Timeframe::Minute {
                match self.lease(&run).await {
                    Ok(acquired) => lease = Some(acquired),
                    Err(error) => {
                        state.set_desired(run.id, DesiredState::Paused).await?;
                        state
                            .set_status(run.id, RunStatus::Paused, Some(error.to_string()), now())
                            .await?;
                    }
                }
            }
            run = self.summary(run.id).await?;
            self.spawn(run, spec, lease);
        }
        Ok(())
    }

    pub async fn start_backtest(
        &self,
        bot: &str,
        request: StartBacktest,
        today: NaiveDate,
    ) -> Result<RunSummary, RunnerError> {
        let spec = self.bot(bot)?;
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
        let symbols = gate_symbols(&self.inner.market, &config).await?;
        let market = &self.inner.market;
        let gaps = market
            .coverage_gaps(symbols.clone(), Timeframe::Day, period.from, period.to)
            .await?;
        if !gaps.is_empty() {
            return Err(RunnerError::CoverageGaps(gaps));
        }
        let warnings = market
            .unexplained_moves(symbols, period.from, period.to)
            .await?;
        let run = self
            .create(config, &spec, RunStatus::Queued, warnings)
            .await?;
        self.spawn(run.clone(), spec, None);
        Ok(run)
    }

    pub async fn go_live(&self, bot: &str) -> Result<RunSummary, RunnerError> {
        let spec = self.bot(bot)?;
        if let Some(run) = self.live_run(bot).await? {
            if run.desired != DesiredState::Paused {
                return Err(RunnerError::AlreadyLive(bot.to_owned()));
            }
            self.inner
                .state
                .set_desired(run.id, DesiredState::Running)
                .await?;
            self.inner.steer(run.id, DesiredState::Running);
            return self.summary(run.id).await;
        }
        let today = now().exchange_date();
        let config = self.run_config(
            &spec,
            Mode::Live,
            today,
            None,
            false,
            StartBacktest {
                preset: None,
                period: None,
                params: None,
                start_cash: None,
                cost: None,
                risk: None,
                seed: None,
                allow_holdout: false,
            },
        )?;
        let run = match self
            .create(config, &spec, RunStatus::Starting, Vec::new())
            .await
        {
            Err(RunnerError::State(StateError::LiveRunExists(bot))) => {
                return Err(RunnerError::AlreadyLive(bot));
            }
            created => created?,
        };
        let lease = if run.config.timeframe == Timeframe::Minute {
            match self.lease(&run).await {
                Ok(lease) => Some(lease),
                Err(error) => {
                    self.conclude(run.id, RunStatus::Failed, Some(error.to_string()))
                        .await?;
                    return Err(error);
                }
            }
        } else {
            None
        };
        self.spawn(run.clone(), spec, lease);
        Ok(run)
    }

    pub async fn pause(&self, bot: &str) -> Result<RunSummary, RunnerError> {
        let run = self
            .live_run(bot)
            .await?
            .ok_or_else(|| RunnerError::NotLive(bot.to_owned()))?;
        if run.desired == DesiredState::Running {
            self.inner
                .state
                .set_desired(run.id, DesiredState::Paused)
                .await?;
            self.inner.steer(run.id, DesiredState::Paused);
        }
        self.summary(run.id).await
    }

    pub async fn cancel(&self, run_id: RunId) -> Result<RunSummary, RunnerError> {
        let run = self.summary(run_id).await?;
        if run.status.is_finished() {
            return Ok(run);
        }
        self.inner
            .state
            .set_desired(run_id, DesiredState::Stopped)
            .await?;
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

    pub async fn benchmark(&self, run: &RunSummary) -> Result<Vec<SparkPoint>, RunnerError> {
        report::benchmark(&self.inner.market, run, now().exchange_date()).await
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

    fn bot(&self, name: &str) -> Result<BotSpec, RunnerError> {
        self.bots()
            .bots
            .into_iter()
            .find(|spec| spec.name == name)
            .ok_or_else(|| RunnerError::UnknownBot(name.to_owned()))
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
        params.extend(request.params.unwrap_or_default());
        let start_cash = request.start_cash.unwrap_or(defaults.start_cash);
        if start_cash <= Decimal::ZERO {
            return Err(RunnerError::InvalidRequest(
                "start cash must be positive".to_owned(),
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
            cost: request.cost.unwrap_or(defaults.cost),
            risk: request.risk.unwrap_or(defaults.risk),
            seed,
            benchmark: manifest.benchmark.clone(),
            holdout,
        })
    }

    async fn lease(&self, run: &RunSummary) -> Result<FeedLease, RunnerError> {
        let symbols = universe_at(&self.inner.market, &run.config).await?;
        Ok(self.inner.feed.acquire(run.id, &run.bot, symbols).await?)
    }

    async fn create(
        &self,
        config: RunConfig,
        spec: &BotSpec,
        status: RunStatus,
        warnings: Vec<DataWarning>,
    ) -> Result<RunSummary, RunnerError> {
        let inner = &self.inner;
        let instruments = inner.market.instruments().await?;
        let mut badges = Vec::new();
        if survivorship_biased(&config.universe, &instruments) {
            badges.push(RunBadge::SurvivorshipBiased);
        }
        if config.holdout {
            badges.push(RunBadge::Holdout);
        }
        if !warnings.is_empty() {
            badges.push(RunBadge::DataWarnings);
        }
        let created_at = now();
        let data_watermark = inner.market.watermarks().borrow().get(config.timeframe);
        let run_id = inner
            .state
            .create_run(NewRun {
                data_watermark,
                config,
                status,
                desired: DesiredState::Running,
                bot_hash: spec.content_hash.clone(),
                engine_version: ENGINE_VERSION.to_owned(),
                badges,
                created_at,
            })
            .await?;
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
        if !events.is_empty() {
            inner.state.append_events(run_id, events).await?;
        }
        self.summary(run_id).await
    }

    async fn summary(&self, run_id: RunId) -> Result<RunSummary, RunnerError> {
        self.inner
            .state
            .run(run_id)
            .await?
            .ok_or(RunnerError::UnknownRun(run_id))
    }

    async fn live_run(&self, bot: &str) -> Result<Option<RunSummary>, RunnerError> {
        Ok(self
            .inner
            .state
            .runs(RunFilter {
                bot: Some(bot.to_owned()),
                mode: Some(Mode::Live),
                statuses: ACTIVE.to_vec(),
                limit: Some(1),
                ..RunFilter::default()
            })
            .await?
            .pop())
    }

    async fn conclude(
        &self,
        run_id: RunId,
        status: RunStatus,
        message: Option<String>,
    ) -> Result<(), RunnerError> {
        drive::conclude(&self.inner, run_id, status, message, None).await?;
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

async fn universe_at(market: &MarketStore, config: &RunConfig) -> Result<Vec<Symbol>, MarketError> {
    match &config.universe {
        UniverseSpec::Symbols { symbols } => Ok(symbols.clone()),
        UniverseSpec::TopDollarVolume { top_dollar_volume } => {
            market
                .top_dollar_volume(*top_dollar_volume, config.start, config.cost.adv_sessions)
                .await
        }
    }
}

pub(crate) async fn gate_symbols(
    market: &MarketStore,
    config: &RunConfig,
) -> Result<Vec<Symbol>, MarketError> {
    let mut symbols = universe_at(market, config).await?;
    symbols.push(config.benchmark.clone());
    symbols.sort();
    symbols.dedup();
    Ok(symbols)
}

pub(crate) fn now() -> Timestamp {
    Timestamp::from_datetime(Utc::now()).unwrap_or_default()
}

fn survivorship_biased(universe: &UniverseSpec, instruments: &[Instrument]) -> bool {
    match universe {
        UniverseSpec::Symbols { .. } => true,
        UniverseSpec::TopDollarVolume { .. } => !instruments
            .iter()
            .any(|instrument| !instrument.active || instrument.last_session.is_some()),
    }
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
    }
}
