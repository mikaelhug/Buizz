use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::WebSocket;
use buizz_core::api::{RunSummary, SpineEvent};
use buizz_core::ledger::LedgerEntry;
use buizz_core::market::{Bar, Session, Symbol, Timeframe, Watermarks};
use buizz_core::order::{Order, OrderRequest, OrderSize, OrderUpdate, RejectReason};
use buizz_core::protocol::{
    Ack, BarBuffer, BarBufferError, EndReason, ErrorCode, Holdings, MAX_STATE_BYTES, Ready, Step,
    StepEvent,
};
use buizz_core::run::{
    DesiredState, EquityPoint, LogLevel, Mode, RiskLimit, RunEvent, RunEventKind, RunId, RunStatus,
};
use buizz_core::time::Timestamp;
use buizz_core::visibility::order_session;
use buizz_engine::{Cycle, ENGINE_VERSION, Engine, EngineError, Outbox};
use buizz_feed::FeedLease;
use buizz_ingest::IngestError;
use buizz_market::MarketError;
use buizz_state::{RunOutcome, StateError, StepCommit};
use buizz_supervisor::{
    BotProcess, BotSpec, LiveProcess, OutputStream, ProcessEvent, SupervisorError, bot_hash,
};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use tokio::sync::{mpsc, watch};

use crate::commit::Committer;
use crate::link::{ACK_TIMEOUT, BotLink, LinkError};
use crate::timeline::{Point, Timeline};
use crate::{Inner, gate_symbols, now, report};

const EXIT_GRACE: Duration = Duration::from_secs(5);
const IDLE_RECHECK: Duration = Duration::from_secs(60);
const EVENT_PAGE: u32 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Control {
    pub desired: DesiredState,
    pub shutdown: bool,
}

pub(crate) struct Launch {
    pub inner: Arc<Inner>,
    pub run: RunSummary,
    pub spec: BotSpec,
    pub control: watch::Sender<Control>,
    pub receiver: watch::Receiver<Control>,
    pub sockets: mpsc::Receiver<WebSocket>,
}

enum Exit {
    Completed,
    Cancelled,
    Shutdown,
    Failed(String),
}

type Flow<T> = Result<T, Exit>;

macro_rules! fails_with {
    ($($error:ty),*) => {
        $(impl From<$error> for Exit {
            fn from(error: $error) -> Self {
                Self::Failed(error.to_string())
            }
        })*
    };
}

fails_with!(
    StateError,
    MarketError,
    EngineError,
    SupervisorError,
    IngestError,
    crate::RunnerError
);

enum Process {
    Backtest(BotProcess),
    Live(LiveProcess),
}

impl Process {
    fn events(&mut self) -> &mut mpsc::Receiver<ProcessEvent> {
        match self {
            Self::Backtest(process) => process.events(),
            Self::Live(process) => process.events(),
        }
    }

    async fn stop(self) {
        let stopped = match self {
            Self::Backtest(mut process) => {
                if tokio::time::timeout(EXIT_GRACE, process.wait())
                    .await
                    .is_ok()
                {
                    return;
                }
                process.kill().await
            }
            Self::Live(process) => process.stop().await,
        };
        if let Err(error) = stopped {
            tracing::warn!(%error, "stopping a bot process failed");
        }
    }
}

#[derive(Default)]
struct Pending {
    ledger: Vec<LedgerEntry>,
    orders: Vec<Order>,
    equity_points: Vec<EquityPoint>,
    events: Vec<RunEvent>,
    bot_state: Option<Vec<u8>>,
    dirty: bool,
}

#[derive(Default)]
struct Frame {
    bars: BarBuffer,
    columns: HashMap<Symbol, u16>,
    events: Vec<StepEvent>,
    due: bool,
    holdings: Option<Holdings>,
    universe: Option<Vec<Symbol>>,
}

impl Frame {
    fn push(&mut self, bar: &Bar) -> Flow<()> {
        let buffer = &mut self.bars;
        let index = match self.columns.get(&bar.symbol) {
            Some(index) => *index,
            None => {
                let index = u16::try_from(buffer.symbols.len())
                    .map_err(|_| Exit::Failed(BarBufferError::TooManySymbols.to_string()))?;
                buffer.symbols.push(bar.symbol.clone());
                self.columns.insert(bar.symbol.clone(), index);
                index
            }
        };
        let float = |value: Decimal| value.to_f64().unwrap_or(f64::NAN).to_le_bytes();
        buffer.symbol_index.extend_from_slice(&index.to_le_bytes());
        buffer
            .start
            .extend_from_slice(&bar.start.nanos().to_le_bytes());
        buffer.open.extend_from_slice(&float(bar.open));
        buffer.high.extend_from_slice(&float(bar.high));
        buffer.low.extend_from_slice(&float(bar.low));
        buffer.close.extend_from_slice(&float(bar.close));
        buffer
            .volume
            .extend_from_slice(&(bar.volume as f64).to_le_bytes());
        Ok(())
    }

    fn discard(&mut self) {
        self.bars = BarBuffer::default();
        self.columns.clear();
        self.events.clear();
        self.due = false;
    }
}

pub(crate) async fn run(mut launch: Launch) {
    let inner = Arc::clone(&launch.inner);
    let run_id = launch.run.id;
    let token = inner.issue_token(run_id);
    let (exit, closing) = match prepare(&inner, &launch.run, &mut launch.receiver).await {
        Err(exit) => (exit, None),
        Ok(slot) => {
            let opened = Driver::open(launch, &token).await;
            match opened {
                Err(exit) => (exit, None),
                Ok(mut driver) => (driver.drive().await, Some((driver, slot))),
            }
        }
    };
    let (status, message) = match exit {
        Exit::Shutdown => (None, None),
        Exit::Completed => (Some(RunStatus::Completed), None),
        Exit::Cancelled => (Some(RunStatus::Cancelled), None),
        Exit::Failed(message) => (Some(RunStatus::Failed), Some(message)),
    };
    let (message, result_hash) = match closing {
        Some((driver, _slot)) => driver.close(status, message).await,
        None => (message, None),
    };
    if let Some(status) = status
        && let Err(error) = conclude(&inner, run_id, status, message, result_hash).await
    {
        tracing::error!(run = %run_id, %error, "cannot record the end of a run");
    }
    inner.forget(run_id, &token);
}

async fn prepare(
    inner: &Inner,
    run: &RunSummary,
    control: &mut watch::Receiver<Control>,
) -> Flow<Option<tokio::sync::OwnedSemaphorePermit>> {
    if run.mode == Mode::Live {
        return Ok(None);
    }
    if run.config.timeframe == Timeframe::Minute {
        let config = &run.config;
        let end = config.end.unwrap_or(config.start);
        let symbols = gate_symbols(&inner.market, config).await?;
        tokio::select! {
            ensured = inner.ingest.ensure_minute_coverage(&symbols, config.start, end) => ensured?,
            exit = interrupted(control) => return Err(exit),
        }
        let gaps = inner
            .market
            .coverage_gaps(symbols, Timeframe::Minute, config.start, end)
            .await?;
        if let Some(gap) = gaps.first() {
            return Err(Exit::Failed(format!(
                "market data has {} minute-bar gaps, the first {} from {} to {}",
                gaps.len(),
                gap.symbol,
                gap.from,
                gap.to
            )));
        }
    }
    tokio::select! {
        slot = inner.supervisor.backtest_slot() => Ok(Some(slot)),
        exit = interrupted(control) => Err(exit),
    }
}

async fn interrupted(control: &mut watch::Receiver<Control>) -> Exit {
    loop {
        let current = *control.borrow_and_update();
        if current.shutdown {
            return Exit::Shutdown;
        }
        if current.desired == DesiredState::Stopped {
            return Exit::Cancelled;
        }
        if control.changed().await.is_err() {
            return Exit::Shutdown;
        }
    }
}

pub(crate) async fn conclude(
    inner: &Inner,
    run_id: RunId,
    status: RunStatus,
    message: Option<String>,
    result_hash: Option<String>,
) -> Result<(), StateError> {
    let Some(run) = inner.state.run(run_id).await? else {
        return Err(StateError::UnknownRun(run_id));
    };
    let metrics = match status {
        RunStatus::Completed => match report::metrics(&inner.market, &inner.state, &run).await {
            Ok(metrics) => Some(metrics),
            Err(error) => {
                tracing::warn!(run = %run_id, %error, "cannot compute run metrics");
                None
            }
        },
        _ => None,
    };
    let at = now();
    inner
        .state
        .finish_run(
            run_id,
            RunOutcome {
                status,
                finished_at: at,
                result_hash,
                metrics,
                message: message.clone(),
            },
        )
        .await?;
    let seq = last_event_seq(inner, run_id).await? + 1;
    let event = RunEvent {
        seq,
        at,
        clock: run.clock,
        kind: RunEventKind::Status { status, message },
    };
    inner
        .state
        .append_events(run_id, vec![event.clone()])
        .await?;
    inner.broadcast(SpineEvent::Log { run_id, event });
    inner.broadcast(SpineEvent::RunStatus {
        run_id,
        bot: run.bot,
        status,
    });
    Ok(())
}

async fn last_event_seq(inner: &Inner, run_id: RunId) -> Result<u64, StateError> {
    let mut last = 0;
    loop {
        let page = inner
            .state
            .events(run_id, (last > 0).then_some(last), EVENT_PAGE)
            .await?;
        match page.last() {
            Some(event) => last = event.seq,
            None => return Ok(last),
        }
    }
}

struct Driver {
    inner: Arc<Inner>,
    run: RunSummary,
    spec: BotSpec,
    token: String,
    control_tx: watch::Sender<Control>,
    control: watch::Receiver<Control>,
    sockets: mpsc::Receiver<WebSocket>,
    watermarks: watch::Receiver<Watermarks>,
    lease: Option<FeedLease>,
    engine: Engine,
    timeline: Timeline,
    committer: Committer,
    pending: Pending,
    frame: Frame,
    after: Timestamp,
    clock: Option<Timestamp>,
    session: Option<Session>,
    link: Option<BotLink>,
    process: Option<Process>,
    status: RunStatus,
    paused: bool,
    halted: Option<String>,
    event_seq: u64,
    step_seq: u64,
    bot_state: Option<Vec<u8>>,
    arrived_while_deciding: Option<(Timestamp, Timestamp)>,
}

impl Driver {
    async fn open(launch: Launch, token: &str) -> Flow<Self> {
        let Launch {
            inner,
            run,
            spec,
            control: control_tx,
            receiver: control,
            sockets,
        } = launch;
        let config = &run.config;
        let timeline = Timeline::open(inner.market.clone(), config).await?;
        let checkpoint = inner.state.checkpoint(run.id).await?;
        if checkpoint.is_some()
            && run.mode == Mode::Backtest
            && let Some(message) = engine_changed(&run)
        {
            return Err(Exit::Failed(message));
        }
        let (engine, after, clock, last_step, bot_state) = match checkpoint {
            Some(checkpoint) => (
                Engine::restore(config, &checkpoint.engine_checkpoint)?,
                checkpoint.clock,
                Some(checkpoint.clock),
                checkpoint.step_seq,
                checkpoint.bot_state,
            ),
            None => {
                let (after, start) = match run.mode {
                    Mode::Backtest => (
                        Timestamp::MIN,
                        timeline
                            .next_point(Timestamp::MIN)
                            .map_or(run.created_at, |point| point.session.open),
                    ),
                    Mode::Live => (run.created_at, run.created_at),
                };
                (Engine::new(config, start), after, None, 0, None)
            }
        };
        Ok(Self {
            token: token.to_owned(),
            control_tx,
            control,
            sockets,
            watermarks: inner.market.watermarks(),
            lease: None,
            engine,
            timeline,
            committer: Committer::spawn(inner.state.clone(), last_step),
            pending: Pending::default(),
            frame: Frame::default(),
            after,
            clock,
            session: None,
            link: None,
            process: None,
            status: run.status,
            paused: false,
            halted: None,
            event_seq: last_event_seq(&inner, run.id).await?,
            step_seq: last_step,
            bot_state,
            arrived_while_deciding: None,
            spec,
            run,
            inner,
        })
    }

    fn live(&self) -> bool {
        self.run.mode == Mode::Live
    }

    fn streams(&self) -> bool {
        self.live() && self.run.config.timeframe == Timeframe::Minute
    }

    fn wanted(&self) -> BTreeSet<Symbol> {
        self.timeline
            .session()
            .universe
            .iter()
            .cloned()
            .chain(self.held())
            .collect()
    }

    async fn lease_stream(&mut self) -> Flow<()> {
        if !self.streams() || self.paused {
            return Ok(());
        }
        let wanted = self.wanted();
        let leased = match self.lease.as_mut() {
            Some(lease) => lease.cover(wanted),
            None => self
                .inner
                .feed
                .acquire(self.run.id, &self.run.bot, wanted)
                .map(|lease| self.lease = Some(lease)),
        };
        match leased {
            Ok(()) => Ok(()),
            Err(error) => {
                let message = format!("{error}; the run is paused");
                self.pause(EndReason::Failed, Some(message)).await
            }
        }
    }

    fn lease_orders(&mut self, orders: &[OrderRequest]) -> BTreeSet<Symbol> {
        if !self.streams() {
            return BTreeSet::new();
        }
        let symbols: BTreeSet<Symbol> = orders
            .iter()
            .map(|order| order.symbol.clone())
            .filter(|symbol| self.timeline.instrument(symbol).is_some())
            .collect();
        let Some(lease) = self.lease.as_mut() else {
            return BTreeSet::new();
        };
        let mut uncovered = BTreeSet::new();
        for symbol in symbols {
            if lease.symbols().contains(&symbol) {
                continue;
            }
            let wanted: Vec<Symbol> = lease
                .symbols()
                .iter()
                .cloned()
                .chain([symbol.clone()])
                .collect();
            if lease.cover(wanted).is_err() {
                uncovered.insert(symbol);
            }
        }
        uncovered
    }

    async fn outdated(&mut self) -> Flow<Option<String>> {
        if let Some(message) = engine_changed(&self.run) {
            return Ok(Some(message));
        }
        let Some(recorded) = self.run.reproducibility.as_ref() else {
            return Ok(None);
        };
        let dir = self.spec.dir.clone();
        let current = tokio::task::spawn_blocking(move || bot_hash(&dir))
            .await
            .expect("hashing a bot does not panic")?;
        Ok((current != recorded.bot_hash).then(|| {
            format!(
                "the code of bot {} changed since this run started; stop the run and start a new one",
                self.run.bot
            )
        }))
    }

    async fn enter_next_session(&mut self) -> Flow<()> {
        if self.timeline.session().date.is_none()
            && let Some(point) = self.timeline.next_point(self.after)
        {
            let held = self.held();
            self.timeline.enter(&point.session, &held).await?;
        }
        Ok(())
    }

    async fn drive(&mut self) -> Exit {
        match self.advance().await {
            Ok(()) => Exit::Completed,
            Err(exit) => exit,
        }
    }

    async fn advance(&mut self) -> Flow<()> {
        self.reconcile().await?;
        loop {
            if self.control.has_changed().unwrap_or(true) {
                self.reconcile().await?;
            }
            let next = self.timeline.next_point(self.after);
            if !self.live() {
                let Some(point) = next else { return Ok(()) };
                self.cycle(&point).await?;
                if self.frame.due {
                    self.wake().await?;
                }
                continue;
            }
            match next.filter(|point| self.processable(point)) {
                Some(_) if !self.paused && self.link.is_none() => self.await_bot().await?,
                Some(point) => {
                    self.cycle(&point).await?;
                    if self.frame.due && self.link.is_some() && !self.arrived_while_deciding() {
                        self.wake().await?;
                    }
                }
                None if self.frame.due && (self.paused || self.link.is_some()) => {
                    self.wake().await?;
                }
                None => {
                    if self.frame.due {
                        self.publish_equity();
                    }
                    self.commit(true).await?;
                    if next.is_none() {
                        self.timeline.refresh().await?;
                    }
                    self.idle().await?;
                }
            }
        }
    }

    fn arrived_while_deciding(&self) -> bool {
        self.arrived_while_deciding.is_some_and(|(asked, decided)| {
            self.timeline
                .next_point(self.after)
                .is_some_and(|next| asked < next.visible_at && next.visible_at <= decided)
        })
    }

    fn visible_through(&self) -> Option<Timestamp> {
        self.watermarks.borrow().get(self.run.config.timeframe)
    }

    fn processable(&mut self, point: &Point) -> bool {
        self.watermarks
            .borrow_and_update()
            .get(self.run.config.timeframe)
            .is_some_and(|through| point.visible_at <= through)
    }

    async fn reconcile(&mut self) -> Flow<()> {
        let control = *self.control.borrow_and_update();
        if control.shutdown {
            return Err(Exit::Shutdown);
        }
        match control.desired {
            DesiredState::Stopped => Err(Exit::Cancelled),
            DesiredState::Paused if !self.paused => self.pause(EndReason::Paused, None).await,
            DesiredState::Running if self.paused => {
                self.paused = false;
                self.frame.discard();
                self.launch().await
            }
            DesiredState::Running if self.process.is_none() => self.launch().await,
            _ => Ok(()),
        }
    }

    async fn launch(&mut self) -> Flow<()> {
        if let Some(message) = self.outdated().await? {
            return match self.run.mode {
                Mode::Backtest => Err(Exit::Failed(message)),
                Mode::Live => self.pause(EndReason::Failed, Some(message)).await,
            };
        }
        self.enter_next_session().await?;
        self.lease_stream().await?;
        if self.paused {
            return Ok(());
        }
        let launch = buizz_supervisor::Launch {
            run_id: self.run.id,
            token: self.token.clone(),
            bot: self.spec.clone(),
        };
        let supervisor = &self.inner.supervisor;
        self.process = Some(match self.run.mode {
            Mode::Backtest => Process::Backtest(supervisor.spawn(&launch).await?),
            Mode::Live => Process::Live(supervisor.keep_alive(launch)),
        });
        self.set_status(RunStatus::Starting, None).await
    }

    async fn pause(&mut self, reason: EndReason, message: Option<String>) -> Flow<()> {
        self.paused = true;
        self.lease = None;
        self.engine.cancel_all(self.after);
        self.absorb();
        self.frame.discard();
        if let Some(link) = self.link.take() {
            link.end(reason, message.clone()).await;
        }
        if let Some(process) = self.process.take() {
            process.stop().await;
        }
        self.control_tx.send_if_modified(|control| {
            let changed = control.desired != DesiredState::Paused;
            control.desired = DesiredState::Paused;
            changed
        });
        self.control.borrow_and_update();
        self.inner
            .state
            .set_desired(self.run.id, DesiredState::Paused)
            .await?;
        match self.run.mode {
            Mode::Live => self.set_status(RunStatus::Paused, message).await,
            Mode::Backtest => {
                if let Some(message) = message {
                    self.note(RunEventKind::Log {
                        level: LogLevel::Warn,
                        message: message.clone(),
                    });
                    self.halted = Some(message);
                }
                Ok(())
            }
        }
    }

    async fn cycle(&mut self, point: &Point) -> Flow<()> {
        if self.session.map(|session| session.date) != Some(point.session.date) {
            if self.live() {
                self.timeline.refresh().await?;
            }
            let held = self.held();
            self.timeline.enter(&point.session, &held).await?;
            self.lease_stream().await?;
        }
        let visible_through = self.live().then(|| self.visible_through()).flatten();
        let bars = self.timeline.bars(point, visible_through).await?;
        let data = self.timeline.session();
        self.engine.run_cycle(&Cycle {
            session: &point.session,
            start: point.start,
            visible_at: point.visible_at,
            bars: &bars,
            actions: &data.actions,
            average_daily_volume: &data.average_daily_volume,
            delisted: &data.delisted,
        });
        self.after = point.visible_at;
        self.clock = Some(point.visible_at);
        self.session = Some(point.session);
        for bar in &bars {
            self.frame.push(bar)?;
        }
        if self
            .run
            .config
            .wake
            .is_due(point.visible_at, &point.session)
        {
            self.frame.due = true;
        }
        if let Some(breach) = self.absorb()
            && !self.paused
        {
            let message = format!(
                "drawdown {:.2}% breached the {:.2}% limit; the bot is paused",
                breach.0 * Decimal::ONE_HUNDRED,
                breach.1 * Decimal::ONE_HUNDRED
            );
            self.pause(EndReason::RiskLimit, Some(message)).await?;
        }
        Ok(())
    }

    fn held(&self) -> BTreeSet<Symbol> {
        let holdings = self.engine.holdings();
        holdings
            .positions
            .into_iter()
            .map(|position| position.symbol)
            .chain(holdings.working.into_iter().map(|order| order.symbol))
            .collect()
    }

    fn absorb(&mut self) -> Option<(Decimal, Decimal)> {
        let Outbox {
            ledger,
            orders,
            fills,
            step_events,
            equity_points,
            breaches,
        } = self.engine.drain();
        let run_id = self.run.id;
        for fill in fills {
            self.inner.broadcast(SpineEvent::Fill { run_id, fill });
        }
        for order in &orders {
            self.inner.broadcast(SpineEvent::Order {
                run_id,
                order: OrderUpdate::from(order),
            });
        }
        for point in &equity_points {
            self.inner.broadcast(SpineEvent::Equity {
                run_id,
                bot: self.run.bot.clone(),
                at: point.at,
                equity: point.equity,
            });
        }
        self.pending.ledger.extend(ledger);
        self.pending.orders.extend(orders);
        self.pending.equity_points.extend(equity_points);
        self.pending.dirty = true;
        if !self.paused {
            self.frame.events.extend(step_events);
        }
        let mut drawdown = None;
        for breach in breaches {
            if breach.limit == RiskLimit::MaxDrawdown {
                drawdown = Some((breach.value, breach.threshold));
            }
            self.note(RunEventKind::RiskBreach {
                limit: breach.limit,
                value: breach.value,
                threshold: breach.threshold,
            });
        }
        drawdown
    }

    async fn wake(&mut self) -> Flow<()> {
        self.drain_process().await?;
        if self.live() {
            self.publish_equity();
        }
        if self.paused {
            self.frame.discard();
            return self.commit(false).await;
        }
        if self.link.is_none() {
            if self.live() {
                return Ok(());
            }
            self.await_bot().await?;
        }
        self.step().await
    }

    fn publish_equity(&self) {
        if let Some(at) = self.clock {
            self.inner.broadcast(SpineEvent::Equity {
                run_id: self.run.id,
                bot: self.run.bot.clone(),
                at,
                equity: self.engine.portfolio().equity(),
            });
        }
    }

    async fn await_bot(&mut self) -> Flow<()> {
        let deadline = tokio::time::Instant::now() + ACK_TIMEOUT;
        while self.link.is_none() {
            tokio::select! {
                socket = self.sockets.recv() => self.attach(socket).await?,
                event = next_process_event(&mut self.process) => self.on_process(event).await?,
                _ = self.control.changed() => self.reconcile().await?,
                _ = tokio::time::sleep_until(deadline) => {
                    let message = format!(
                        "the bot did not connect within {} s",
                        ACK_TIMEOUT.as_secs()
                    );
                    if self.live() {
                        return self.pause(EndReason::Failed, Some(message)).await;
                    }
                    return Err(Exit::Failed(message));
                }
            }
            if self.paused {
                return Ok(());
            }
        }
        Ok(())
    }

    async fn idle(&mut self) -> Flow<()> {
        tokio::select! {
            _ = self.watermarks.changed() => {}
            _ = self.control.changed() => self.reconcile().await?,
            socket = self.sockets.recv() => self.attach(socket).await?,
            event = next_process_event(&mut self.process) => self.on_process(event).await?,
            _ = tokio::time::sleep(IDLE_RECHECK) => {}
        }
        Ok(())
    }

    async fn attach(&mut self, socket: Option<WebSocket>) -> Flow<()> {
        let Some(socket) = socket else {
            return Err(Exit::Shutdown);
        };
        if self.paused {
            return Ok(());
        }
        let ready = self.ready().await?;
        let universe = ready.universe.clone();
        match BotLink::handshake(socket, ready).await {
            Ok(link) => {
                self.link = Some(link);
                self.arrived_while_deciding = None;
                self.frame.holdings = None;
                self.frame.universe = Some(universe);
                self.set_status(RunStatus::Running, None).await
            }
            Err(error) if self.live() => {
                self.record(RunEventKind::Log {
                    level: LogLevel::Warn,
                    message: format!("bot handshake failed: {error}"),
                })
                .await
            }
            Err(error) => Err(Exit::Failed(format!("bot handshake failed: {error}"))),
        }
    }

    async fn ready(&mut self) -> Flow<Ready> {
        self.enter_next_session().await?;
        let config = &self.run.config;
        let portfolio = self.engine.portfolio();
        Ok(Ready {
            run_id: self.run.id,
            bot: self.run.bot.clone(),
            mode: self.run.mode,
            timeframe: config.timeframe,
            wake: config.wake,
            params: config.params.clone(),
            universe: self.timeline.session().universe.clone(),
            start: config.start,
            end: config.end,
            seed: config.seed,
            state: self.bot_state.clone().map(Into::into),
            cash: portfolio.cash,
            equity: portfolio.equity(),
            holdings: self.engine.holdings(),
        })
    }

    async fn step(&mut self) -> Flow<()> {
        let Some(session) = self.session else {
            return Ok(());
        };
        let clock = self.after;
        let holdings = self.engine.holdings();
        let universe = &self.timeline.session().universe;
        let portfolio = self.engine.portfolio();
        let universe_changed = self.frame.universe.as_ref() != Some(universe);
        self.step_seq += 1;
        let step = Step {
            seq: self.step_seq,
            clock,
            session,
            universe: universe_changed.then(|| universe.clone()),
            bars: self.frame.bars.clone(),
            events: self.frame.events.clone(),
            cash: portfolio.cash,
            equity: portfolio.equity(),
            holdings: (self.frame.holdings.as_ref() != Some(&holdings)).then(|| holdings.clone()),
        };
        let sent_universe = universe_changed.then(|| universe.clone());
        let market = self.inner.market.clone();
        let asked = self.visible_through();
        let Some(link) = self.link.as_mut() else {
            return Ok(());
        };
        let answered = {
            let mut answer = std::pin::pin!(link.step(step, &market));
            loop {
                tokio::select! {
                    answered = &mut answer => break Some(answered),
                    changed = self.control.changed() => {
                        let control = *self.control.borrow();
                        if changed.is_err() || control.shutdown || control.desired != DesiredState::Running {
                            break None;
                        }
                    }
                }
            }
        };
        match answered {
            None => self.reconcile().await,
            Some(Ok(ack)) => {
                self.arrived_while_deciding = asked.zip(self.visible_through());
                self.frame.discard();
                self.frame.holdings = Some(holdings);
                if sent_universe.is_some() {
                    self.frame.universe = sent_universe;
                }
                self.accept(ack).await
            }
            Some(Err(LinkError::Closed)) => {
                self.link = None;
                if self.live() {
                    return Ok(());
                }
                self.drain_process().await?;
                Err(Exit::Failed("the bot closed its connection".to_owned()))
            }
            Some(Err(error)) => self.bot_fault(error.to_string()).await,
        }
    }

    async fn bot_fault(&mut self, message: String) -> Flow<()> {
        self.link = None;
        if self.live() {
            self.pause(EndReason::Failed, Some(message)).await
        } else {
            Err(Exit::Failed(message))
        }
    }

    async fn accept(&mut self, ack: Ack) -> Flow<()> {
        let Ack {
            orders,
            cancels,
            state,
            logs,
            ..
        } = ack;
        if let Some(state) = state {
            if state.len() > MAX_STATE_BYTES {
                let message = format!(
                    "bot state is {} bytes, the limit is {MAX_STATE_BYTES}",
                    state.len()
                );
                if let Some(link) = self.link.as_mut() {
                    link.fail(ErrorCode::StateTooLarge, message.clone()).await;
                }
                return self.bot_fault(message).await;
            }
            let state = state.into_vec();
            self.bot_state = Some(state.clone());
            self.pending.bot_state = Some(state);
        }
        for line in logs {
            self.note(RunEventKind::Log {
                level: line.level,
                message: line.message,
            });
        }
        let unleased = self.lease_orders(&orders);
        let (accepted, rejected): (Vec<OrderRequest>, Vec<(OrderRequest, RejectReason)>) =
            orders.into_iter().fold(
                (Vec::new(), Vec::new()),
                |(mut accepted, mut rejected), order| {
                    let unleased = unleased
                        .contains(&order.symbol)
                        .then_some(RejectReason::NoStreamSlot);
                    match self.screen(&order).or(unleased) {
                        Some(reason) => rejected.push((order, reason)),
                        None => accepted.push(order),
                    }
                    (accepted, rejected)
                },
            );
        for (order, reason) in rejected {
            self.note(RunEventKind::OrderRejected {
                client_id: order.client_id,
                symbol: order.symbol,
                reason,
            });
        }
        let clock = self.after;
        let session = order_session(self.run.config.timeframe, clock, self.timeline.calendar())
            .or(self.session.as_ref())
            .map(|session| session.date)
            .unwrap_or_else(|| clock.exchange_date());
        let ordered = !accepted.is_empty();
        self.engine.accept(clock, session, accepted, cancels);
        self.absorb();
        if ordered {
            let held = self.held();
            self.timeline.track(held);
        }
        self.lease_stream().await?;
        self.commit(false).await
    }

    fn screen(&self, order: &OrderRequest) -> Option<RejectReason> {
        let Some(instrument) = self.timeline.instrument(&order.symbol) else {
            return Some(RejectReason::UnknownSymbol);
        };
        if instrument.shortable {
            return None;
        }
        let holdings = self.engine.holdings();
        let working: i64 = holdings
            .working
            .iter()
            .filter(|working| working.symbol == order.symbol)
            .map(|working| {
                working
                    .side
                    .signed(working.qty.saturating_sub(working.filled_qty))
            })
            .sum();
        let held = self.engine.portfolio().qty(&order.symbol) + working;
        let short = match order.size {
            OrderSize::Qty { qty } => held + qty < 0,
            OrderSize::TargetWeight { weight } => weight.is_sign_negative() && !weight.is_zero(),
        };
        short.then_some(RejectReason::NotShortable)
    }

    async fn commit(&mut self, force: bool) -> Flow<()> {
        if !self.pending.dirty || !(force || self.committer.idle()) {
            return Ok(());
        }
        let Some(clock) = self.clock else {
            let events = std::mem::take(&mut self.pending.events);
            if !events.is_empty() {
                self.inner.state.append_events(self.run.id, events).await?;
            }
            return Ok(());
        };
        let pending = std::mem::take(&mut self.pending);
        self.committer
            .send(StepCommit {
                run_id: self.run.id,
                step_seq: 0,
                clock,
                equity: self.engine.portfolio().equity(),
                ledger: pending.ledger,
                orders: pending.orders,
                equity_points: pending.equity_points,
                events: pending.events,
                engine_checkpoint: self.engine.checkpoint(),
                bot_state: pending.bot_state,
            })
            .await?;
        Ok(())
    }

    async fn drain_process(&mut self) -> Flow<()> {
        loop {
            let event = match self
                .process
                .as_mut()
                .map(|process| process.events().try_recv())
            {
                Some(Ok(event)) => Some(event),
                Some(Err(mpsc::error::TryRecvError::Disconnected)) => None,
                Some(Err(mpsc::error::TryRecvError::Empty)) | None => return Ok(()),
            };
            self.on_process(event).await?;
        }
    }

    async fn on_process(&mut self, event: Option<ProcessEvent>) -> Flow<()> {
        let kind = match event {
            Some(ProcessEvent::Started { pid }) => RunEventKind::Log {
                level: LogLevel::Info,
                message: format!("bot process {pid} started"),
            },
            Some(ProcessEvent::Line { stream, line }) => RunEventKind::Log {
                level: match stream {
                    OutputStream::Stdout => LogLevel::Info,
                    OutputStream::Stderr => LogLevel::Warn,
                },
                message: line,
            },
            Some(ProcessEvent::Restarting { attempt, delay }) => RunEventKind::Log {
                level: LogLevel::Warn,
                message: format!(
                    "restarting the bot in {} s (attempt {attempt})",
                    delay.as_secs()
                ),
            },
            Some(ProcessEvent::Exited { code }) => {
                self.link = None;
                self.record(RunEventKind::BotExited { code }).await?;
                if !self.live() {
                    self.process = None;
                    return Err(Exit::Failed(match code {
                        Some(code) => format!("the bot exited with code {code}"),
                        None => "the bot was killed by a signal".to_owned(),
                    }));
                }
                return Ok(());
            }
            None => {
                self.process = None;
                if self.live() && !self.paused {
                    let message = "the bot crashed too often; the run is paused".to_owned();
                    return self.pause(EndReason::Failed, Some(message)).await;
                }
                return Ok(());
            }
        };
        self.note(kind);
        Ok(())
    }

    fn event(&mut self, kind: RunEventKind) -> RunEvent {
        self.event_seq += 1;
        let event = RunEvent {
            seq: self.event_seq,
            at: now(),
            clock: self.clock,
            kind,
        };
        self.inner.broadcast(SpineEvent::Log {
            run_id: self.run.id,
            event: event.clone(),
        });
        event
    }

    fn note(&mut self, kind: RunEventKind) {
        let event = self.event(kind);
        self.pending.events.push(event);
        self.pending.dirty = true;
    }

    async fn record(&mut self, kind: RunEventKind) -> Flow<()> {
        let event = self.event(kind);
        self.inner
            .state
            .append_events(self.run.id, vec![event])
            .await?;
        Ok(())
    }

    async fn set_status(&mut self, status: RunStatus, message: Option<String>) -> Flow<()> {
        if status == self.status && message.is_none() {
            return Ok(());
        }
        self.status = status;
        self.inner
            .state
            .set_status(self.run.id, status, message.clone(), now())
            .await?;
        self.record(RunEventKind::Status { status, message })
            .await?;
        self.inner.broadcast(SpineEvent::RunStatus {
            run_id: self.run.id,
            bot: self.run.bot.clone(),
            status,
        });
        Ok(())
    }

    async fn close(
        mut self,
        status: Option<RunStatus>,
        message: Option<String>,
    ) -> (Option<String>, Option<String>) {
        let reason = match status {
            Some(RunStatus::Completed) => EndReason::Completed,
            Some(RunStatus::Failed) => EndReason::Failed,
            _ => EndReason::Stopped,
        };
        if let Some(link) = self.link.take() {
            link.end(reason, message.clone()).await;
        }
        if let Some(process) = self.process.take() {
            process.stop().await;
        }
        if matches!(status, Some(RunStatus::Cancelled | RunStatus::Failed)) {
            self.engine.cancel_all(self.after);
            self.absorb();
        }
        let mut message = message.or_else(|| self.halted.clone());
        let flushed = match self.commit(true).await {
            Ok(()) => self
                .committer
                .finish()
                .await
                .map_err(|error| error.to_string()),
            Err(Exit::Failed(error)) => Err(error),
            Err(_) => Ok(()),
        };
        if let Err(error) = flushed {
            tracing::error!(run = %self.run.id, %error, "cannot persist the last steps of a run");
            message.get_or_insert(error);
        }
        let result_hash = (status == Some(RunStatus::Completed)).then(|| self.engine.result_hash());
        (message, result_hash)
    }
}

fn engine_changed(run: &RunSummary) -> Option<String> {
    let recorded = &run.reproducibility.as_ref()?.engine_version;
    (recorded != ENGINE_VERSION).then(|| {
        format!(
            "the engine changed from {recorded} to {ENGINE_VERSION} since this run started; stop the run and start a new one"
        )
    })
}

async fn next_process_event(process: &mut Option<Process>) -> Option<ProcessEvent> {
    match process {
        Some(process) => process.events().recv().await,
        None => std::future::pending().await,
    }
}
