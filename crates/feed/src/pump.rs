use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use buizz_core::api::FeedSlots;
use buizz_core::market::{Bar, DataSource, Dataset, Feed, Symbol, Timeframe};
use buizz_core::time::Timestamp;
use buizz_market::MarketStore;
use buizz_sources::alpaca::{AlpacaClient, BarsRequest, SIP_DELAY};
use buizz_sources::alpaca_stream::{self, StreamConfig, StreamEvent, StreamHandle};
use chrono::{NaiveTime, TimeDelta, Utc};
use futures::future::BoxFuture;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::{FeedError, Inner};

const MINUTE: TimeDelta = TimeDelta::minutes(1);
const LATE_BAR_GRACE: TimeDelta = TimeDelta::seconds(10);
const BACKFILL_LOOKBACK_LIMIT: TimeDelta = TimeDelta::days(7);
const RETRY_INITIAL: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(60);
const STORE_RETRY: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Problems {
    stream: Option<String>,
    subscription: Option<String>,
    calendar: Option<String>,
    backfill: Option<String>,
    store: Option<String>,
}

impl Problems {
    fn describe(&self) -> Option<String> {
        let present: Vec<&str> = [
            &self.stream,
            &self.subscription,
            &self.calendar,
            &self.backfill,
            &self.store,
        ]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect();
        (!present.is_empty()).then(|| present.join("; "))
    }
}

struct Backfill {
    symbols: BTreeSet<Symbol>,
    from: Timestamp,
    to: Timestamp,
    attempt: u32,
    bars: BoxFuture<'static, Result<Vec<Bar>, FeedError>>,
}

enum Step {
    Stream(Option<StreamEvent>),
    Leases,
    Backfilled(Result<Vec<Bar>, FeedError>),
    Wake,
}

struct Pump {
    inner: Arc<Inner>,
    alpaca: AlpacaClient,
    stream: StreamHandle,
    subscribed: BTreeSet<Symbol>,
    connected: bool,
    settled_through: Option<Timestamp>,
    published_through: Option<Timestamp>,
    pending: BTreeMap<Timestamp, BTreeMap<Symbol, Bar>>,
    unwritten: VecDeque<(DataSource, Vec<Bar>)>,
    store_retry_at: Option<Instant>,
    backfill: Option<Backfill>,
    problems: Problems,
}

pub(crate) async fn run(inner: Arc<Inner>, config: StreamConfig, alpaca: AlpacaClient) {
    let (stream, mut events) = alpaca_stream::spawn(config);
    let mut leases = inner.slots.subscribe();
    let mut pump = Pump {
        inner,
        alpaca,
        stream,
        subscribed: BTreeSet::new(),
        connected: false,
        settled_through: None,
        published_through: None,
        pending: BTreeMap::new(),
        unwritten: VecDeque::new(),
        store_retry_at: None,
        backfill: None,
        problems: Problems::default(),
    };
    match stored_through(&pump.inner.market).await {
        Ok(through) => pump.settled_through = through,
        Err(error) => pump.problems.store = Some(error.to_string()),
    }
    pump.reconcile(&mut leases).await;
    loop {
        pump.settle_minutes(now());
        pump.flush().await;
        pump.publish();
        let wake = pump.wake_at();
        let step = tokio::select! {
            event = events.recv() => Step::Stream(event),
            Ok(()) = leases.changed() => Step::Leases,
            bars = backfilled(&mut pump.backfill) => Step::Backfilled(bars),
            () = sleep_until(wake) => Step::Wake,
        };
        match step {
            Step::Stream(Some(event)) => pump.on_event(event).await,
            Step::Stream(None) => {
                pump.connected = false;
                pump.flush().await;
                pump.publish();
                return;
            }
            Step::Leases => pump.reconcile(&mut leases).await,
            Step::Backfilled(bars) => pump.on_backfilled(bars),
            Step::Wake => {}
        }
    }
}

impl Pump {
    fn healthy(&self) -> bool {
        self.connected && self.backfill.is_none()
    }

    async fn on_event(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::Connected => {
                self.connected = true;
                self.problems.stream = None;
                let now = minute_floor(now());
                match self.settled_through {
                    Some(through) => self.request_backfill(self.subscribed.clone(), through, now),
                    None => self.settled_through = Some(now),
                }
            }
            StreamEvent::Subscribed { .. } => self.problems.subscription = None,
            StreamEvent::Bar(bar) => self.on_bar(bar).await,
            StreamEvent::Gap { from, to } => {
                self.request_backfill(self.subscribed.clone(), from, to);
            }
            StreamEvent::SymbolLimitExceeded => {
                self.problems.subscription =
                    Some("Alpaca refused the subscription: symbol limit exceeded".to_owned());
            }
            StreamEvent::Disconnected { reason } => {
                self.connected = false;
                self.problems.stream = Some(format!("stream disconnected: {reason}"));
            }
        }
    }

    async fn reconcile(&mut self, leases: &mut watch::Receiver<FeedSlots>) {
        let wanted: BTreeSet<Symbol> = leases
            .borrow_and_update()
            .holders
            .iter()
            .flat_map(|holder| holder.symbols.iter().cloned())
            .collect();
        if wanted == self.subscribed {
            return;
        }
        let removed = self.subscribed.difference(&wanted).cloned().collect();
        let added: BTreeSet<Symbol> = wanted.difference(&self.subscribed).cloned().collect();
        let outcome = self
            .stream
            .unsubscribe(removed)
            .await
            .and(self.stream.subscribe(added.iter().cloned().collect()).await);
        if let Err(error) = outcome {
            self.problems.subscription = Some(error.to_string());
        }
        self.subscribed = wanted;
        if !added.is_empty() {
            self.backfill_session(added).await;
        }
    }

    async fn backfill_session(&mut self, symbols: BTreeSet<Symbol>) {
        let now = now();
        let date = now.exchange_date();
        let calendar = match self.inner.market.calendar(date, date).await {
            Ok(calendar) => calendar,
            Err(error) => {
                self.problems.calendar = Some(error.to_string());
                return;
            }
        };
        if let Some(session) = calendar.session(date) {
            let to = self.settled_through.unwrap_or_else(|| minute_floor(now));
            self.request_backfill(symbols, session.open, to.min(session.close));
        }
    }

    async fn on_bar(&mut self, bar: Bar) {
        let date = bar.start.exchange_date();
        let calendar = self
            .inner
            .market
            .calendar(date, date)
            .await
            .unwrap_or_default();
        let Some(session) = calendar.session(date) else {
            self.problems.calendar = Some(format!(
                "the calendar has no session on {date}, so its live bars are not stored"
            ));
            return;
        };
        self.problems.calendar = None;
        if !(session.open..session.close).contains(&bar.start) {
            return;
        }
        if self
            .settled_through
            .is_some_and(|through| bar.start < through)
        {
            self.queue(DataSource::AlpacaStream, vec![bar]);
        } else {
            self.pending
                .entry(bar.start)
                .or_default()
                .insert(bar.symbol.clone(), bar);
        }
    }

    fn every_subscribed_bar_arrived(&self, minute: Timestamp) -> bool {
        !self.subscribed.is_empty()
            && self.pending.get(&minute).is_some_and(|arrived| {
                self.subscribed
                    .iter()
                    .all(|symbol| arrived.contains_key(symbol))
            })
    }

    fn settle_minutes(&mut self, now: Timestamp) {
        if !self.healthy() {
            return;
        }
        let Some(mut minute) = self.settled_through else {
            return;
        };
        let mut bars = Vec::new();
        while self.every_subscribed_bar_arrived(minute) || quiet_past_grace(minute, now) {
            if let Some(arrived) = self.pending.remove(&minute) {
                bars.extend(arrived.into_values());
            }
            minute = minute + MINUTE;
        }
        self.settled_through = Some(minute);
        self.queue(DataSource::AlpacaStream, bars);
    }

    fn request_backfill(&mut self, symbols: BTreeSet<Symbol>, from: Timestamp, to: Timestamp) {
        let floor = minute_floor(now()) - BACKFILL_LOOKBACK_LIMIT;
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
            self.start_backfill(symbols, from, to, 0);
        }
    }

    fn start_backfill(
        &mut self,
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
            self.alpaca.clone(),
            self.inner.market.clone(),
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

    fn on_backfilled(&mut self, fetched: Result<Vec<Bar>, FeedError>) {
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
                self.queue(DataSource::AlpacaStream, before_range);
                self.queue(DataSource::Alpaca, bars);
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
                self.start_backfill(done.symbols, done.from, done.to, done.attempt + 1);
            }
        }
    }

    fn queue(&mut self, source: DataSource, bars: Vec<Bar>) {
        if !bars.is_empty() {
            self.unwritten.push_back((source, bars));
        }
    }

    async fn flush(&mut self) {
        if self.store_retry_at.is_some_and(|at| Instant::now() < at) {
            return;
        }
        let market = &self.inner.market;
        while let Some((source, bars)) = self.unwritten.front() {
            if let Err(error) = market
                .upsert_bars(*source, Timeframe::Minute, bars.clone())
                .await
            {
                tracing::warn!(%error, "writing live minute bars failed");
                self.problems.store = Some(format!("writing minute bars failed: {error}"));
                self.store_retry_at = Some(Instant::now() + STORE_RETRY);
                return;
            }
            self.unwritten.pop_front();
        }
        self.store_retry_at = None;
        self.problems.store = None;
        let Some(through) = self.settled_through else {
            return;
        };
        if self.published_through >= Some(through) {
            return;
        }
        match market.advance_watermark(Timeframe::Minute, through).await {
            Ok(()) => self.published_through = Some(through),
            Err(error) => self.problems.store = Some(error.to_string()),
        }
    }

    fn publish(&self) {
        let error = self.problems.describe();
        self.inner.slots.send_if_modified(|slots| {
            let changed = slots.connected != self.connected || slots.error != error;
            slots.connected = self.connected;
            slots.error = error;
            changed
        });
    }

    fn wake_at(&self) -> Option<Instant> {
        let settle = self
            .settled_through
            .filter(|_| self.healthy())
            .map(|minute| instant_at(minute + MINUTE + LATE_BAR_GRACE));
        let retry = self.store_retry_at.filter(|_| !self.unwritten.is_empty());
        settle.into_iter().chain(retry).min()
    }
}

fn quiet_past_grace(minute: Timestamp, now: Timestamp) -> bool {
    now >= minute + MINUTE + LATE_BAR_GRACE
}

async fn fetch_minutes(
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
    let sip_until = minute_floor(now() - sip_delay).clamp(from, to);
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

async fn backfilled(backfill: &mut Option<Backfill>) -> Result<Vec<Bar>, FeedError> {
    match backfill {
        Some(backfill) => (&mut backfill.bars).await,
        None => std::future::pending().await,
    }
}

async fn sleep_until(wake: Option<Instant>) {
    match wake {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

fn now() -> Timestamp {
    Timestamp::from_datetime(Utc::now()).expect("current time fits in i64 nanoseconds")
}

fn minute_floor(at: Timestamp) -> Timestamp {
    at - TimeDelta::nanoseconds(at.nanos().rem_euclid(60_000_000_000))
}

fn instant_at(at: Timestamp) -> Instant {
    Instant::now() + (at - now()).to_std().unwrap_or_default()
}
