use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::WebSocket;
use buizz_core::api::{RunSummary, SpineEvent};
use buizz_core::ledger::LedgerEntry;
use buizz_core::market::{Bar, Session, Symbol, Timeframe, Watermarks};
use buizz_core::order::{Order, OrderRequest, OrderUpdate, RejectReason};
use buizz_core::protocol::{
    Ack, BarBuffer, BarBufferError, EndReason, ErrorCode, Holdings, MAX_STATE_BYTES, Ready, Step,
    StepEvent,
};
use buizz_core::run::{
    DRAWDOWN_STOP, DesiredState, EquityPoint, LogLevel, Mode, RunBadge, RunConfig, RunEvent,
    RunEventKind, RunId, RunStatus,
};
use buizz_core::time::Timestamp;
use buizz_core::visibility::{close_auction_session, open_auction_session, order_session};
use buizz_engine::{Cycle, Decision, DrawdownStop, Engine, EngineError, Outbox};
use buizz_feed::FeedLease;
use buizz_ingest::IngestError;
use buizz_market::MarketError;
use buizz_state::{RunOutcome, StateError, StepCommit};
use buizz_supervisor::{
    BotProcess, BotSpec, LiveProcess, OutputStream, ProcessEvent, SupervisorError, bot_hash,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};

use crate::commit::Committer;
use crate::link::{BotLink, LinkError};
use crate::timeline::{Point, Timeline};
use crate::{Inner, gate_symbols, report};

const EXIT_GRACE: Duration = Duration::from_secs(5);
const IDLE_RECHECK: Duration = Duration::from_secs(60);
const EVENT_PAGE: u32 = 1000;
const ORDER_PAGE: u32 = 1000;
const BACKTEST_EQUITY_EVERY: Duration = Duration::from_millis(250);

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
    pub lease: Option<FeedLease>,
}

enum Exit {
    Completed,
    Cancelled,
    Shutdown,
    Failed(String),
    DrawdownStopped,
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

    fn undelivered(&self) -> Option<Vec<u8>> {
        let empty = self.bars.symbol_index.is_empty() && self.events.is_empty() && !self.due;
        (!empty).then(|| {
            rmp_serde::to_vec_named(&Undelivered {
                bars: &self.bars,
                events: &self.events,
                due: self.due,
            })
            .expect("undelivered steps encode as MessagePack")
        })
    }

    fn restore(saved: Option<&[u8]>) -> Flow<Self> {
        let Some(saved) = saved else {
            return Ok(Self::default());
        };
        let restored: Restored = rmp_serde::from_slice(saved)
            .map_err(|error| Exit::Failed(format!("the saved step is unreadable: {error}")))?;
        let columns = restored
            .bars
            .symbols
            .iter()
            .zip(0..)
            .map(|(symbol, index)| (symbol.clone(), index))
            .collect();
        Ok(Self {
            bars: restored.bars,
            columns,
            events: restored.events,
            due: restored.due,
            holdings: None,
            universe: None,
        })
    }
}

#[derive(Serialize)]
struct Undelivered<'a> {
    bars: &'a BarBuffer,
    events: &'a [StepEvent],
    due: bool,
}

#[derive(Deserialize)]
struct Restored {
    bars: BarBuffer,
    events: Vec<StepEvent>,
    due: bool,
}

pub(crate) async fn run(mut launch: Launch) {
    let inner = Arc::clone(&launch.inner);
    let run_id = launch.run.id;
    let token = inner.issue_token(run_id);
    let (exit, closing) = match prepare(&inner, &launch.run, &mut launch.receiver).await {
        Err(exit) => (exit, None),
        Ok((slot, minutes)) => {
            let opened = Driver::open(launch, &token, minutes).await;
            match opened {
                Err(exit) => (exit, None),
                Ok(mut driver) => (driver.drive().await, Some((driver, slot))),
            }
        }
    };
    let (status, message, reason) = match exit {
        Exit::Shutdown => (None, None, EndReason::Stopped),
        Exit::Completed => (Some(RunStatus::Completed), None, EndReason::Completed),
        Exit::Cancelled => (Some(RunStatus::Cancelled), None, EndReason::Stopped),
        Exit::Failed(message) => (Some(RunStatus::Failed), Some(message), EndReason::Failed),
        Exit::DrawdownStopped => (
            Some(RunStatus::Failed),
            Some(DRAWDOWN_STOP.to_owned()),
            EndReason::RiskLimit,
        ),
    };
    let badge = (reason == EndReason::RiskLimit).then_some(RunBadge::DrawdownStop);
    let (message, result_hash) = match closing {
        Some((driver, _slot)) => driver.close(status, reason, message).await,
        None => (message, None),
    };
    if let Some(status) = status
        && let Err(error) = conclude(&inner, run_id, status, message, result_hash, badge).await
    {
        tracing::error!(run = %run_id, %error, "cannot record the end of a run");
    }
    inner.forget(run_id, &token);
}

type Prepared = (Option<tokio::sync::OwnedSemaphorePermit>, BTreeSet<Symbol>);

async fn prepare(
    inner: &Inner,
    run: &RunSummary,
    control: &mut watch::Receiver<Control>,
) -> Flow<Prepared> {
    if run.mode == Mode::Live {
        return Ok((None, BTreeSet::new()));
    }
    let mut minutes = BTreeSet::new();
    if run.config.needs_minute_bars() {
        let config = &run.config;
        let symbols = gate_symbols(&inner.market, config).await?;
        backfill_minutes(inner, config, &symbols, config.start, control).await?;
        minutes.extend(symbols);
    }
    tokio::select! {
        slot = inner.supervisor.backtest_slot() => Ok((Some(slot), minutes)),
        exit = interrupted(control) => Err(exit),
    }
}

async fn backfill_minutes(
    inner: &Inner,
    config: &RunConfig,
    symbols: &[Symbol],
    from: NaiveDate,
    control: &mut watch::Receiver<Control>,
) -> Flow<()> {
    let end = config.end.unwrap_or(config.start);
    tokio::select! {
        ensured = inner.ingest.ensure_minute_coverage(symbols, from, end) => ensured?,
        exit = interrupted(control) => return Err(exit),
    }
    let gaps = inner
        .market
        .coverage_gaps(symbols.to_vec(), Timeframe::Minute, from, end)
        .await?;
    match gaps.first() {
        None => Ok(()),
        Some(gap) => Err(Exit::Failed(format!(
            "market data has {} minute-bar gaps, the first {} from {} to {}",
            gaps.len(),
            gap.symbol,
            gap.from,
            gap.to
        ))),
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
    badge: Option<RunBadge>,
) -> Result<(), StateError> {
    let Some(run) = inner.state.run(run_id).await? else {
        return Err(StateError::UnknownRun(run_id));
    };
    let metrics = match result_hash {
        Some(_) => match report::metrics(&inner.market, &inner.state, &run).await {
            Ok(metrics) => Some(metrics),
            Err(error) => {
                tracing::warn!(run = %run_id, %error, "cannot compute run metrics");
                None
            }
        },
        None => None,
    };
    let at = Timestamp::now();
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
                badge,
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

async fn client_ids(inner: &Inner, run_id: RunId) -> Result<BTreeSet<String>, StateError> {
    let mut ids = BTreeSet::new();
    let mut after = None;
    loop {
        let page = inner.state.orders(run_id, after, ORDER_PAGE).await?;
        let Some(last) = page.last() else {
            return Ok(ids);
        };
        after = Some(last.id.0);
        ids.extend(page.into_iter().map(|order| order.client_id));
    }
}

async fn last_event_seq(inner: &Inner, run_id: RunId) -> Result<u64, StateError> {
    let mut last = 0;
    loop {
        let page = inner
            .state
            .events(run_id, (last > 0).then_some(last), EVENT_PAGE, Vec::new())
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
    minutes: BTreeSet<Symbol>,
    link: Option<BotLink>,
    process: Option<Process>,
    status: RunStatus,
    paused: bool,
    event_seq: u64,
    step_seq: u64,
    bot_state: Option<Vec<u8>>,
    arrived_while_deciding: Option<(Timestamp, Timestamp)>,
    equity_shown_at: Option<tokio::time::Instant>,
    unshown_equity: Option<EquityPoint>,
}

impl Driver {
    async fn open(launch: Launch, token: &str, minutes: BTreeSet<Symbol>) -> Flow<Self> {
        let Launch {
            inner,
            run,
            spec,
            control: control_tx,
            receiver: control,
            sockets,
            lease,
        } = launch;
        let config = &run.config;
        let timeline = Timeline::open(inner.market.clone(), config).await?;
        let checkpoint = inner.state.checkpoint(run.id).await?;
        let (engine, after, clock, last_step, bot_state, frame) = match checkpoint {
            Some(checkpoint) => (
                Engine::restore(
                    config,
                    &checkpoint.engine_checkpoint,
                    client_ids(&inner, run.id).await?,
                )?,
                checkpoint.clock,
                Some(checkpoint.clock),
                checkpoint.step_seq,
                checkpoint.bot_state,
                Frame::restore(checkpoint.undelivered.as_deref())?,
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
                (
                    Engine::new(config, start),
                    after,
                    None,
                    0,
                    None,
                    Frame::default(),
                )
            }
        };
        let session = clock.and_then(|clock| timeline.calendar().containing(clock).copied());
        Ok(Self {
            token: token.to_owned(),
            control_tx,
            control,
            sockets,
            watermarks: inner.market.watermarks(),
            lease,
            engine,
            timeline,
            committer: Committer::spawn(inner.state.clone(), last_step),
            pending: Pending::default(),
            frame,
            after,
            clock,
            session,
            minutes,
            link: None,
            process: None,
            status: run.status,
            paused: false,
            event_seq: last_event_seq(&inner, run.id).await?,
            step_seq: last_step,
            bot_state,
            arrived_while_deciding: None,
            equity_shown_at: None,
            unshown_equity: None,
            spec,
            run,
            inner,
        })
    }

    fn live(&self) -> bool {
        self.run.mode == Mode::Live
    }

    fn benched(&self) -> bool {
        self.paused || self.engine.drawdown_stop().is_some()
    }

    fn streams(&self) -> bool {
        self.live() && self.run.config.needs_minute_bars()
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
        if !self.streams() || self.paused || self.timeline.session().date.is_none() {
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

    fn recorded_hash(&self) -> &str {
        self.run
            .reproducibility
            .as_ref()
            .map_or(&self.spec.content_hash, |recorded| &recorded.bot_hash)
    }

    fn outdated_message(&self) -> String {
        format!(
            "the code of bot {} changed since this run started; stop the run and start a new one",
            self.run.bot
        )
    }

    async fn outdated(&mut self) -> Flow<Option<String>> {
        let dir = self.spec.dir.clone();
        let current = tokio::task::spawn_blocking(move || bot_hash(&dir))
            .await
            .expect("hashing a bot does not panic")?;
        Ok((current != self.recorded_hash()).then(|| self.outdated_message()))
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
            Ok(()) if self.engine.drawdown_stop().is_some() => Exit::DrawdownStopped,
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
                Some(_) if !self.benched() && self.link.is_none() => self.await_bot().await?,
                Some(point) => {
                    self.cycle(&point).await?;
                    if self.frame.due && self.link.is_some() && !self.arrived_while_deciding() {
                        self.wake().await?;
                    }
                }
                None if self.frame.due && self.deliverable() => {
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

    fn deliverable(&self) -> bool {
        self.benched() || (self.link.is_some() && self.session.is_some())
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
        let marks = *self.watermarks.borrow_and_update();
        let timeframe = if point.pre_close {
            Timeframe::Minute
        } else {
            self.run.config.timeframe
        };
        let visible = |timeframe, at| marks.get(timeframe).is_some_and(|through| at <= through);
        let awaits_auction = self.run.config.timeframe == Timeframe::Minute
            && point.closes_session()
            && !self.engine.closing_auction(point.session.date).is_empty();
        visible(timeframe, point.visible_at)
            && (!awaits_auction || visible(Timeframe::Day, point.session.close))
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
        if self.engine.drawdown_stop().is_some() {
            self.enter_next_session().await?;
            return self.lease_stream().await;
        }
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
            bot: BotSpec {
                content_hash: self.recorded_hash().to_owned(),
                ..self.spec.clone()
            },
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
            Mode::Backtest => Ok(()),
        }
    }

    async fn dismiss_bot(&mut self) {
        self.frame.discard();
        if let Some(link) = self.link.take() {
            link.end(EndReason::RiskLimit, Some(DRAWDOWN_STOP.to_owned()))
                .await;
        }
        if let Some(process) = self.process.take() {
            process.stop().await;
        }
    }

    async fn cycle(&mut self, point: &Point) -> Flow<()> {
        let entering = self.session.map(|session| session.date) != Some(point.session.date);
        if entering || self.timeline.session().date != Some(point.session.date) {
            if self.live() {
                self.timeline.refresh().await?;
                if entering {
                    self.timeline.leave_session();
                }
            }
            let held = self.held();
            self.timeline.enter(&point.session, &held).await?;
            self.backfill_newcomers(point.session.date).await?;
            self.lease_stream().await?;
        }
        let bars = if point.pre_close {
            self.timeline.session_so_far(point).await?
        } else {
            let visible_through = self.live().then(|| self.visible_through()).flatten();
            self.timeline.bars(point, visible_through).await?
        };
        let auction = match self.run.config.timeframe {
            Timeframe::Minute if point.closes_session() => {
                let symbols = self.engine.closing_auction(point.session.date);
                match symbols.is_empty() {
                    true => Vec::new(),
                    false => self.timeline.official(&point.session, symbols).await?,
                }
            }
            _ => Vec::new(),
        };
        let data = self.timeline.session();
        let cycle = Cycle {
            session: &point.session,
            start: point.start,
            visible_at: point.visible_at,
            bars: &bars,
            official: match self.run.config.timeframe {
                Timeframe::Day => &bars,
                Timeframe::Minute => &auction,
            },
            actions: &data.actions,
            average_daily_volume: &data.average_daily_volume,
            delisted: &data.delisted,
            risk_free_rate: data.risk_free_rate,
        };
        if point.pre_close {
            self.engine.pre_close(&cycle);
        } else {
            self.engine.run_cycle(&cycle);
        }
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
        self.absorb();
        match self.engine.drawdown_stop() {
            Some(DrawdownStop::Flat) => Err(Exit::DrawdownStopped),
            Some(DrawdownStop::Closing) => {
                self.dismiss_bot().await;
                Ok(())
            }
            None => Ok(()),
        }
    }

    async fn quote_untracked(
        &mut self,
        orders: &[OrderRequest],
        unleased: &BTreeSet<Symbol>,
        session: &Session,
    ) -> Flow<()> {
        let untracked: BTreeSet<Symbol> = orders
            .iter()
            .map(|order| &order.symbol)
            .filter(|symbol| {
                self.timeline.instrument(symbol).is_some()
                    && !unleased.contains(*symbol)
                    && !self.timeline.tracked().contains(*symbol)
            })
            .cloned()
            .collect();
        if untracked.is_empty() {
            return Ok(());
        }
        self.backfill_unfetched(&untracked, session.date).await?;
        let closes = self
            .timeline
            .last_closes(session, self.after, untracked.into_iter().collect())
            .await?;
        self.engine.quote(closes);
        Ok(())
    }

    async fn backfill_newcomers(&mut self, from: NaiveDate) -> Flow<()> {
        let tracked = self.timeline.tracked().clone();
        self.backfill_unfetched(&tracked, from).await
    }

    async fn backfill_unfetched(
        &mut self,
        symbols: &BTreeSet<Symbol>,
        from: NaiveDate,
    ) -> Flow<()> {
        if self.live() || !self.run.config.needs_minute_bars() {
            return Ok(());
        }
        let newcomers: Vec<Symbol> = symbols.difference(&self.minutes).cloned().collect();
        if newcomers.is_empty() {
            return Ok(());
        }
        backfill_minutes(
            &self.inner,
            &self.run.config,
            &newcomers,
            from,
            &mut self.control,
        )
        .await?;
        self.minutes.extend(newcomers);
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

    fn absorb(&mut self) {
        let Outbox {
            ledger,
            orders,
            fills,
            step_events,
            equity_points,
            breaches,
            warnings,
            rejections,
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
        if self.live() {
            for point in &equity_points {
                self.broadcast_equity(point.at, point.equity);
            }
        } else if let Some(point) = equity_points.last() {
            self.unshown_equity = Some(point.clone());
            self.show_backtest_equity(false);
        }
        self.pending.ledger.extend(ledger);
        self.pending.orders.extend(orders);
        self.pending.equity_points.extend(equity_points);
        self.pending.dirty = true;
        if !self.benched() {
            self.frame.events.extend(step_events);
        }
        for order in rejections {
            if let Some(reason) = order.reject_reason {
                self.note(RunEventKind::OrderRejected {
                    client_id: order.client_id,
                    symbol: order.symbol,
                    reason,
                });
            }
        }
        for message in warnings {
            self.note(RunEventKind::DataWarning { message });
        }
        for breach in breaches {
            self.note(RunEventKind::RiskBreach {
                limit: breach.limit,
                value: breach.value,
                threshold: breach.threshold,
            });
        }
    }

    async fn wake(&mut self) -> Flow<()> {
        self.drain_process().await?;
        if self.live() {
            self.publish_equity();
        }
        if self.benched() {
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
            self.broadcast_equity(at, self.engine.portfolio().equity());
        }
    }

    fn show_backtest_equity(&mut self, last: bool) {
        let due = last
            || self
                .equity_shown_at
                .is_none_or(|shown| shown.elapsed() >= BACKTEST_EQUITY_EVERY);
        if !due {
            return;
        }
        if let Some(point) = self.unshown_equity.take() {
            self.broadcast_equity(point.at, point.equity);
            self.equity_shown_at = Some(tokio::time::Instant::now());
        }
    }

    fn broadcast_equity(&self, at: Timestamp, equity: Decimal) {
        self.inner.broadcast(SpineEvent::Equity {
            run_id: self.run.id,
            bot: self.run.bot.clone(),
            at,
            equity,
        });
    }

    async fn await_bot(&mut self) -> Flow<()> {
        let timeout = self.spec.manifest.step_timeout();
        let deadline = tokio::time::Instant::now() + timeout;
        while self.link.is_none() {
            tokio::select! {
                socket = self.sockets.recv() => self.attach(socket).await?,
                event = next_process_event(&mut self.process) => self.on_process(event).await?,
                _ = self.control.changed() => self.reconcile().await?,
                _ = tokio::time::sleep_until(deadline) => {
                    let message = format!(
                        "the bot did not connect within {} s",
                        timeout.as_secs()
                    );
                    if self.live() {
                        return self.pause(EndReason::Failed, Some(message)).await;
                    }
                    return Err(Exit::Failed(message));
                }
            }
            if self.benched() {
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
        if self.benched() {
            return Ok(());
        }
        let ready = self.ready().await?;
        let universe = ready.universe.clone();
        match BotLink::handshake(socket, ready, self.spec.manifest.step_timeout()).await {
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
        let clock = self.after;
        let calendar = self.timeline.calendar();
        let Some(current) = self.session else {
            return Ok(());
        };
        let next = |found: Option<&Session>| found.copied().unwrap_or(current);
        let session = next(order_session(self.run.config.timeframe, clock, calendar));
        let on_open = next(open_auction_session(clock, calendar));
        let on_close = next(close_auction_session(clock, calendar));
        self.quote_untracked(&orders, &unleased, &current).await?;
        let decision = Decision {
            at: clock,
            session: &session,
            on_open: &on_open,
            on_close: &on_close,
        };
        let ordered = !orders.is_empty();
        let timeline = &self.timeline;
        self.engine.accept(&decision, orders, cancels, |symbol| {
            match timeline.instrument(symbol) {
                None => Err(RejectReason::UnknownSymbol),
                Some(_) if unleased.contains(symbol) => Err(RejectReason::NoStreamSlot),
                Some(instrument) => Ok(instrument),
            }
        });

        self.absorb();
        if ordered {
            let held = self.held();
            self.timeline.track(held).await?;
            self.backfill_newcomers(current.date).await?;
        }
        self.lease_stream().await?;
        self.commit(false).await
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
                undelivered: self.frame.undelivered(),
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
            Some(ProcessEvent::Outdated) => {
                self.link = None;
                let message = self.outdated_message();
                return self.pause(EndReason::Failed, Some(message)).await;
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
            at: Timestamp::now(),
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
            .set_status(self.run.id, status, message.clone(), Timestamp::now())
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
        reason: EndReason,
        mut message: Option<String>,
    ) -> (Option<String>, Option<String>) {
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
        self.show_backtest_equity(true);
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
        let concluded =
            status.is_some() && matches!(reason, EndReason::Completed | EndReason::RiskLimit);
        let result_hash = concluded.then(|| self.engine.result_hash());
        (message, result_hash)
    }
}

async fn next_process_event(process: &mut Option<Process>) -> Option<ProcessEvent> {
    match process {
        Some(process) => process.events().recv().await,
        None => std::future::pending().await,
    }
}
