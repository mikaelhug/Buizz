use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use buizz_core::api::FeedSlots;
use buizz_core::market::{Bar, DataSource, Dataset, Feed, Symbol, Timeframe};
use buizz_core::time::Timestamp;
use buizz_market::MarketStore;
use buizz_sources::alpaca::{AlpacaClient, BarsRequest, SIP_DELAY};
use buizz_sources::alpaca_stream::{self, StreamConfig, StreamEvent, StreamHandle};
use chrono::{NaiveTime, TimeDelta};
use futures::future::BoxFuture;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt, StreamMap};

use crate::{FeedError, Inner};

pub const LATE_BAR_GRACE: TimeDelta = TimeDelta::seconds(10);
const MINUTE: TimeDelta = TimeDelta::minutes(1);
const BACKFILL_LOOKBACK_LIMIT: TimeDelta = TimeDelta::days(7);
const RETRY_INITIAL: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(60);
const STORE_RETRY: Duration = Duration::from_secs(5);

type Events = Pin<Box<dyn Stream<Item = Option<StreamEvent>> + Send>>;

fn describe<'a>(problems: impl IntoIterator<Item = &'a Option<String>>) -> Option<String> {
    let present: Vec<&str> = problems.into_iter().flatten().map(String::as_str).collect();
    (!present.is_empty()).then(|| present.join("; "))
}

#[derive(Default)]
struct Problems {
    stream: Option<String>,
    subscription: Option<String>,
    backfill: Option<String>,
}

struct Backfill {
    symbols: BTreeSet<Symbol>,
    from: Timestamp,
    to: Timestamp,
    attempt: u32,
    bars: BoxFuture<'static, Result<Vec<Bar>, FeedError>>,
}

enum Step {
    Stream(usize, Option<StreamEvent>),
    Leases,
    Backfilled(usize, Result<Vec<Bar>, FeedError>),
    Wake,
}

struct Rest {
    alpaca: AlpacaClient,
    market: MarketStore,
}

#[derive(Default)]
struct Store {
    unwritten: VecDeque<(DataSource, Vec<Bar>)>,
    retry_at: Option<Instant>,
    published_through: Option<Timestamp>,
    problem: Option<String>,
}

impl Store {
    fn queue(&mut self, source: DataSource, bars: Vec<Bar>) {
        if !bars.is_empty() {
            self.unwritten.push_back((source, bars));
        }
    }
}

struct Connection {
    stream: StreamHandle,
    subscribed: BTreeSet<Symbol>,
    connected: bool,
    closed: bool,
    settled_through: Option<Timestamp>,
    pending: BTreeMap<Timestamp, BTreeMap<Symbol, Bar>>,
    backfill: Option<Backfill>,
    problems: Problems,
}

struct Pump {
    inner: Arc<Inner>,
    rest: Rest,
    connections: Vec<Connection>,
    store: Store,
    calendar_problem: Option<String>,
}

pub(crate) async fn run(inner: Arc<Inner>, streams: Vec<StreamConfig>, alpaca: AlpacaClient) {
    let mut events: StreamMap<usize, Events> = StreamMap::new();
    let mut connections = Vec::new();
    for (index, config) in streams.into_iter().enumerate() {
        let (stream, receiver) = alpaca_stream::spawn(config);
        let ended = ReceiverStream::new(receiver)
            .map(Some)
            .chain(tokio_stream::once(None));
        events.insert(index, Box::pin(ended));
        connections.push(Connection::new(stream));
    }
    let mut leases = inner.slots.subscribe();
    let mut pump = Pump {
        rest: Rest {
            alpaca,
            market: inner.market.clone(),
        },
        inner,
        connections,
        store: Store::default(),
        calendar_problem: None,
    };
    match stored_through(&pump.rest.market).await {
        Ok(through) => {
            for connection in &mut pump.connections {
                connection.settled_through = through;
            }
        }
        Err(error) => pump.store.problem = Some(error.to_string()),
    }
    pump.reconcile(&mut leases).await;
    loop {
        pump.settle_minutes(Timestamp::now());
        pump.flush().await;
        pump.publish();
        let wake = pump.wake_at();
        let step = tokio::select! {
            Some((index, event)) = events.next() => Step::Stream(index, event),
            Ok(()) = leases.changed() => Step::Leases,
            (index, bars) = backfilled(&mut pump.connections) => Step::Backfilled(index, bars),
            () = sleep_until(wake) => Step::Wake,
        };
        match step {
            Step::Stream(index, Some(event)) => pump.on_event(index, event).await,
            Step::Stream(index, None) => pump.on_closed(index),
            Step::Leases => pump.reconcile(&mut leases).await,
            Step::Backfilled(index, bars) => {
                let connection = &mut pump.connections[index];
                connection.on_backfilled(bars, &pump.rest, &mut pump.store);
            }
            Step::Wake => {}
        }
    }
}

impl Pump {
    async fn on_event(&mut self, index: usize, event: StreamEvent) {
        let connection = &mut self.connections[index];
        match event {
            StreamEvent::Connected => {
                connection.connected = true;
                connection.problems.stream = None;
                let now = minute_floor(Timestamp::now());
                match connection.settled_through {
                    Some(through) => {
                        let symbols = connection.subscribed.clone();
                        connection.request_backfill(&self.rest, symbols, through, now);
                    }
                    None => connection.settled_through = Some(now),
                }
            }
            StreamEvent::Subscribed { .. } => connection.problems.subscription = None,
            StreamEvent::Bar(bar) => self.on_bar(index, bar).await,
            StreamEvent::Gap { from, to } => {
                let symbols = connection.subscribed.clone();
                connection.request_backfill(&self.rest, symbols, from, to);
            }
            StreamEvent::SymbolLimitExceeded => {
                connection.problems.subscription =
                    Some("Alpaca refused the subscription: symbol limit exceeded".to_owned());
            }
            StreamEvent::Disconnected { reason } => {
                connection.connected = false;
                connection.problems.stream = Some(format!("stream disconnected: {reason}"));
            }
        }
    }

    fn on_closed(&mut self, index: usize) {
        let connection = &mut self.connections[index];
        connection.connected = false;
        connection.closed = true;
        self.inner.close_connection(index);
    }

    async fn reconcile(&mut self, leases: &mut watch::Receiver<FeedSlots>) {
        leases.mark_unchanged();
        for (index, wanted) in self.inner.placement().into_iter().enumerate() {
            let connection = &mut self.connections[index];
            if wanted == connection.subscribed {
                continue;
            }
            let removed = connection.subscribed.difference(&wanted).cloned().collect();
            let added: BTreeSet<Symbol> =
                wanted.difference(&connection.subscribed).cloned().collect();
            let outcome = connection.stream.unsubscribe(removed).await.and(
                connection
                    .stream
                    .subscribe(added.iter().cloned().collect())
                    .await,
            );
            if let Err(error) = outcome {
                connection.problems.subscription = Some(error.to_string());
            }
            connection.subscribed = wanted;
            if !added.is_empty() {
                self.backfill_session(index, added).await;
            }
        }
    }

    async fn backfill_session(&mut self, index: usize, symbols: BTreeSet<Symbol>) {
        let now = Timestamp::now();
        let date = now.exchange_date();
        let calendar = match self.rest.market.calendar(date, date).await {
            Ok(calendar) => calendar,
            Err(error) => {
                self.calendar_problem = Some(error.to_string());
                return;
            }
        };
        if let Some(session) = calendar.session(date) {
            let connection = &mut self.connections[index];
            let to = connection
                .settled_through
                .unwrap_or_else(|| minute_floor(now));
            connection.request_backfill(&self.rest, symbols, session.open, to.min(session.close));
        }
    }

    async fn on_bar(&mut self, index: usize, bar: Bar) {
        let date = bar.start.exchange_date();
        let calendar = self
            .rest
            .market
            .calendar(date, date)
            .await
            .unwrap_or_default();
        let Some(session) = calendar.session(date) else {
            self.calendar_problem = Some(format!(
                "the calendar has no session on {date}, so its live bars are not stored"
            ));
            return;
        };
        self.calendar_problem = None;
        if !(session.open..session.close).contains(&bar.start) {
            return;
        }
        let connection = &mut self.connections[index];
        if connection
            .settled_through
            .is_some_and(|through| bar.start < through)
        {
            self.store.queue(DataSource::AlpacaStream, vec![bar]);
        } else {
            connection
                .pending
                .entry(bar.start)
                .or_default()
                .insert(bar.symbol.clone(), bar);
        }
    }

    fn settle_minutes(&mut self, now: Timestamp) {
        for connection in &mut self.connections {
            let bars = connection.settle(now);
            self.store.queue(DataSource::AlpacaStream, bars);
        }
    }

    fn watermark(&self) -> Option<Timestamp> {
        let open = self
            .connections
            .iter()
            .filter(|connection| !connection.closed);
        let streaming: Vec<Option<Timestamp>> = open
            .clone()
            .filter(|connection| !connection.subscribed.is_empty())
            .map(|connection| connection.settled_through)
            .collect();
        match streaming.into_iter().min() {
            Some(slowest) => slowest,
            None => open
                .filter_map(|connection| connection.settled_through)
                .max(),
        }
    }

    async fn flush(&mut self) {
        let store = &mut self.store;
        if store.retry_at.is_some_and(|at| Instant::now() < at) {
            return;
        }
        let market = &self.rest.market;
        while let Some((source, bars)) = store.unwritten.front() {
            if let Err(error) = market
                .upsert_bars(*source, Timeframe::Minute, bars.clone())
                .await
            {
                tracing::warn!(%error, "writing live minute bars failed");
                store.problem = Some(format!("writing minute bars failed: {error}"));
                store.retry_at = Some(Instant::now() + STORE_RETRY);
                return;
            }
            store.unwritten.pop_front();
        }
        store.retry_at = None;
        store.problem = None;
        let Some(through) = self.watermark() else {
            return;
        };
        let store = &mut self.store;
        if store.published_through >= Some(through) {
            return;
        }
        match market.advance_watermark(Timeframe::Minute, through).await {
            Ok(()) => store.published_through = Some(through),
            Err(error) => store.problem = Some(error.to_string()),
        }
    }

    fn publish(&self) {
        let error = describe([&self.calendar_problem, &self.store.problem]);
        self.inner.slots.send_if_modified(|slots| {
            let mut changed = slots.error != error;
            slots.error = error;
            for (view, connection) in slots.connections.iter_mut().zip(&self.connections) {
                let problems = &connection.problems;
                let error =
                    describe([&problems.stream, &problems.subscription, &problems.backfill]);
                changed |= view.connected != connection.connected || view.error != error;
                view.connected = connection.connected;
                view.error = error;
            }
            changed
        });
    }

    fn wake_at(&self) -> Option<Instant> {
        let retry = self
            .store
            .retry_at
            .filter(|_| !self.store.unwritten.is_empty());
        self.connections
            .iter()
            .filter_map(Connection::settle_at)
            .chain(retry)
            .min()
    }
}

impl Connection {
    fn new(stream: StreamHandle) -> Self {
        Self {
            stream,
            subscribed: BTreeSet::new(),
            connected: false,
            closed: false,
            settled_through: None,
            pending: BTreeMap::new(),
            backfill: None,
            problems: Problems::default(),
        }
    }

    fn healthy(&self) -> bool {
        self.connected && self.backfill.is_none()
    }

    fn every_subscribed_bar_arrived(&self, minute: Timestamp) -> bool {
        !self.subscribed.is_empty()
            && self.pending.get(&minute).is_some_and(|arrived| {
                self.subscribed
                    .iter()
                    .all(|symbol| arrived.contains_key(symbol))
            })
    }

    fn settle(&mut self, now: Timestamp) -> Vec<Bar> {
        let mut bars = Vec::new();
        if !self.healthy() {
            return bars;
        }
        let Some(mut minute) = self.settled_through else {
            return bars;
        };
        while self.every_subscribed_bar_arrived(minute) || quiet_past_grace(minute, now) {
            if let Some(arrived) = self.pending.remove(&minute) {
                bars.extend(arrived.into_values());
            }
            minute = minute + MINUTE;
        }
        self.settled_through = Some(minute);
        bars
    }

    fn settle_at(&self) -> Option<Instant> {
        self.settled_through
            .filter(|_| self.healthy())
            .map(|minute| instant_at(minute + MINUTE + LATE_BAR_GRACE))
    }

    fn request_backfill(
        &mut self,
        rest: &Rest,
        symbols: BTreeSet<Symbol>,
        from: Timestamp,
        to: Timestamp,
    ) {
        let floor = minute_floor(Timestamp::now()) - BACKFILL_LOOKBACK_LIMIT;
        let from = self
            .settled_through
            .map_or(from, |through| from.min(through))
            .max(floor);
        let (symbols, from, to) = match &self.backfill {
            Some(running)
                if running.from <= from
                    && to <= running.to
                    && symbols.is_subset(&running.symbols) =>
            {
                return;
            }
            Some(running) => (
                symbols.union(&running.symbols).cloned().collect(),
                from.min(running.from),
                to.max(running.to),
            ),
            None => (symbols, from, to),
        };
        if from < to {
            self.start_backfill(rest, symbols, from, to, 0);
        }
    }

    fn start_backfill(
        &mut self,
        rest: &Rest,
        symbols: BTreeSet<Symbol>,
        from: Timestamp,
        to: Timestamp,
        attempt: u32,
    ) {
        let delay = match attempt {
            0 => Duration::ZERO,
            _ => RETRY_INITIAL
                .saturating_mul(1 << (attempt - 1).min(6))
                .min(RETRY_MAX),
        };
        let fetch = fetch_minutes(
            rest.alpaca.clone(),
            rest.market.clone(),
            symbols.iter().cloned().collect(),
            from,
            to,
        );
        self.backfill = Some(Backfill {
            symbols,
            from,
            to,
            attempt,
            bars: Box::pin(async move {
                tokio::time::sleep(delay).await;
                fetch.await
            }),
        });
    }

    fn on_backfilled(
        &mut self,
        fetched: Result<Vec<Bar>, FeedError>,
        rest: &Rest,
        store: &mut Store,
    ) {
        let Some(done) = self.backfill.take() else {
            return;
        };
        match fetched {
            Ok(bars) => {
                let later = self.pending.split_off(&done.to);
                let earlier = std::mem::replace(&mut self.pending, later);
                let before_range = earlier
                    .range(..done.from)
                    .flat_map(|(_, arrived)| arrived.values().cloned())
                    .collect();
                store.queue(DataSource::AlpacaStream, before_range);
                store.queue(DataSource::Alpaca, bars);
                self.settled_through = Some(
                    self.settled_through
                        .map_or(done.to, |through| through.max(done.to)),
                );
                self.problems.backfill = None;
            }
            Err(error) => {
                tracing::warn!(%error, from = %done.from, to = %done.to, "minute backfill failed");
                self.problems.backfill = Some(format!(
                    "backfill {}..{} failed: {error}",
                    done.from, done.to
                ));
                self.start_backfill(rest, done.symbols, done.from, done.to, done.attempt + 1);
            }
        }
    }
}

fn quiet_past_grace(minute: Timestamp, now: Timestamp) -> bool {
    now >= minute + MINUTE + LATE_BAR_GRACE
}

pub(crate) async fn fetch_minutes(
    alpaca: AlpacaClient,
    market: MarketStore,
    symbols: Vec<Symbol>,
    from: Timestamp,
    to: Timestamp,
) -> Result<Vec<Bar>, FeedError> {
    if symbols.is_empty() {
        return Ok(Vec::new());
    }
    let calendar = market
        .calendar(from.exchange_date(), to.exchange_date())
        .await?;
    let sip_delay = TimeDelta::from_std(SIP_DELAY).expect("SIP delay fits a time delta");
    let sip_until = minute_floor(Timestamp::now() - sip_delay).clamp(from, to);
    let request = |feed, from, to| BarsRequest {
        symbols: symbols.clone(),
        timeframe: Timeframe::Minute,
        from,
        to,
        feed,
    };
    let mut bars = alpaca
        .bars(&request(Feed::Sip, from, sip_until), &calendar)
        .await?;
    bars.retain(|bar| bar.start < sip_until);
    let recent = alpaca
        .bars(&request(Feed::Iex, sip_until, to), &calendar)
        .await?;
    bars.extend(recent.into_iter().filter(|bar| bar.start < to));
    Ok(bars)
}

async fn stored_through(market: &MarketStore) -> Result<Option<Timestamp>, FeedError> {
    let coverage = market.coverage().await?;
    let minute = coverage
        .iter()
        .filter(|coverage| coverage.dataset == Dataset::Bars1m);
    let Some(last) = minute.clone().map(|coverage| coverage.last).max() else {
        return Ok(None);
    };
    let symbols: BTreeSet<Symbol> = minute
        .filter(|coverage| coverage.last == last)
        .filter_map(|coverage| coverage.symbol.clone())
        .collect();
    let day_start = |date| Timestamp::at_exchange_time(date, NaiveTime::MIN);
    let (Some(from), Some(to)) = (day_start(last), last.succ_opt().and_then(day_start)) else {
        return Ok(None);
    };
    let bars = market
        .bars_between(Timeframe::Minute, symbols.into_iter().collect(), from, to)
        .await?;
    Ok(bars.iter().map(|bar| bar.start + MINUTE).max())
}

async fn backfilled(connections: &mut [Connection]) -> (usize, Result<Vec<Bar>, FeedError>) {
    let running: Vec<_> = connections
        .iter_mut()
        .enumerate()
        .filter_map(|(index, connection)| {
            let backfill = connection.backfill.as_mut()?;
            Some(Box::pin(async move { (index, (&mut backfill.bars).await) }))
        })
        .collect();
    if running.is_empty() {
        return std::future::pending().await;
    }
    futures::future::select_all(running).await.0
}

async fn sleep_until(wake: Option<Instant>) {
    match wake {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

fn minute_floor(at: Timestamp) -> Timestamp {
    at - TimeDelta::nanoseconds(at.nanos().rem_euclid(60_000_000_000))
}

fn instant_at(at: Timestamp) -> Instant {
    Instant::now() + (at - Timestamp::now()).to_std().unwrap_or_default()
}
