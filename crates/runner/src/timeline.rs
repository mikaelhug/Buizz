use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use buizz_core::listing::Listings;
use buizz_core::market::{
    Bar, Calendar, CorporateAction, CorporateActionKind, Instrument, Liquidity, Session, Symbol,
    Timeframe,
};
use buizz_core::run::{Mode, RATE_LOOKBACK_DAYS, RunConfig, UNIVERSE_SESSIONS, UniverseSpec, Wake};
use buizz_core::time::Timestamp;
use buizz_core::visibility::bar_visible_at;
use buizz_market::{MarketError, MarketStore, TIMESTAMP_RESOLUTION};
use chrono::{NaiveDate, TimeDelta};
use rust_decimal::Decimal;
use tokio::task::JoinHandle;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Point {
    pub session: Session,
    pub start: Timestamp,
    pub visible_at: Timestamp,
    pub pre_close: bool,
}

impl Point {
    pub(crate) fn closes_session(&self) -> bool {
        !self.pre_close && self.session.is_closed_at(self.visible_at)
    }
}

#[derive(Debug, Default)]
pub(crate) struct SessionData {
    pub date: Option<NaiveDate>,
    pub universe_at: Vec<Symbol>,
    pub universe: Vec<Symbol>,
    pub actions: Vec<CorporateAction>,
    pub liquidity: BTreeMap<Symbol, Liquidity>,
    pub delisted: Vec<Symbol>,
    pub risk_free_rate: Option<Decimal>,
}

struct Fetched {
    universe: Vec<Symbol>,
    tracked: BTreeSet<Symbol>,
    actions: Vec<CorporateAction>,
    liquidity: BTreeMap<Symbol, Liquidity>,
    risk_free_rate: Option<Decimal>,
}

impl Fetched {
    fn narrow(&mut self, held: &BTreeSet<Symbol>) {
        self.tracked = self.universe.iter().chain(held).cloned().collect();
        let tracked = &self.tracked;
        self.actions
            .retain(|action| tracked.contains(&action.symbol));
        let delivered: Vec<Symbol> = self
            .actions
            .iter()
            .filter(|action| held.contains(&action.symbol))
            .filter_map(|action| match &action.kind {
                CorporateActionKind::NameChange { new_symbol } => Some(new_symbol.clone()),
                kind => kind.delivered().cloned(),
            })
            .collect();
        self.liquidity.retain(|symbol, _| tracked.contains(symbol));
        self.tracked.extend(delivered);
    }
}

type Upcoming = (NaiveDate, JoinHandle<Result<Fetched, MarketError>>);

#[derive(Default)]
struct Loaded {
    symbols: BTreeSet<Symbol>,
    from: Timestamp,
    to: Timestamp,
    bars: VecDeque<Bar>,
}

pub(crate) struct Timeline {
    market: MarketStore,
    timeframe: Timeframe,
    wake: Wake,
    universe: UniverseSpec,
    adv_sessions: u32,
    prefetch_sessions: bool,
    start: NaiveDate,
    end: Option<NaiveDate>,
    calendar: Calendar,
    instruments: BTreeMap<Symbol, Instrument>,
    listings: Arc<Listings>,
    session: SessionData,
    upcoming: Option<Upcoming>,
    tracked: BTreeSet<Symbol>,
    loaded: Loaded,
    prefetch: Option<JoinHandle<Result<Loaded, MarketError>>>,
}

impl Timeline {
    pub(crate) async fn open(market: MarketStore, config: &RunConfig) -> Result<Self, MarketError> {
        let mut timeline = Self {
            market,
            timeframe: config.timeframe,
            wake: config.wake,
            universe: config.universe.clone(),
            adv_sessions: config.cost.adv_sessions,
            prefetch_sessions: config.mode == Mode::Backtest,
            start: config.start,
            end: config.end,
            calendar: Calendar::default(),
            instruments: BTreeMap::new(),
            listings: Arc::default(),
            session: SessionData::default(),
            upcoming: None,
            tracked: BTreeSet::new(),
            loaded: Loaded::default(),
            prefetch: None,
        };
        timeline.refresh().await?;
        Ok(timeline)
    }

    pub(crate) async fn refresh(&mut self) -> Result<(), MarketError> {
        self.calendar = self.market.calendar(NaiveDate::MIN, NaiveDate::MAX).await?;
        self.instruments = self
            .market
            .instruments()
            .await?
            .into_iter()
            .map(|instrument| (instrument.symbol.clone(), instrument))
            .collect();
        self.listings = self.market.listings();
        Ok(())
    }

    pub(crate) fn calendar(&self) -> &Calendar {
        &self.calendar
    }

    pub(crate) fn instrument(&self, symbol: &Symbol) -> Option<&Instrument> {
        let key = match self.session.date {
            Some(date) => self.listings.listing(symbol, date)?,
            None => symbol.clone(),
        };
        self.instruments.get(&key)
    }

    fn ended(&self, symbol: &Symbol, listed_on: NaiveDate, by: NaiveDate) -> bool {
        self.listings
            .listing(symbol, listed_on)
            .and_then(|key| self.instruments.get(&key))
            .and_then(|instrument| instrument.last_session)
            .is_some_and(|last| last < by)
    }

    pub(crate) fn tracked(&self) -> &BTreeSet<Symbol> {
        &self.tracked
    }

    pub(crate) fn session(&self) -> &SessionData {
        &self.session
    }

    pub(crate) fn next_point(&self, after: Timestamp) -> Option<Point> {
        let sessions = self.calendar.sessions();
        let index =
            sessions.partition_point(|session| session.date < self.start || session.close <= after);
        let session = *sessions.get(index)?;
        if self.end.is_some_and(|end| session.date > end) {
            return None;
        }
        if self.timeframe == Timeframe::Day
            && let Some(at) = self.wake.before_close(&session)
            && after < at
        {
            return Some(Point {
                session,
                start: session.open,
                visible_at: at,
                pre_close: true,
            });
        }
        let start = match self.timeframe {
            Timeframe::Day => session.open,
            Timeframe::Minute if after < session.open => session.open,
            Timeframe::Minute => {
                session.open + TimeDelta::minutes((after - session.open).num_minutes())
            }
        };
        Some(Point {
            session,
            start,
            visible_at: bar_visible_at(self.timeframe, start, &session),
            pre_close: false,
        })
    }

    pub(crate) fn leave_session(&mut self) {
        self.session = SessionData::default();
    }

    pub(crate) async fn enter(
        &mut self,
        session: &Session,
        held: &BTreeSet<Symbol>,
    ) -> Result<(), MarketError> {
        if self.session.date == Some(session.date) {
            return Ok(());
        }
        let upcoming = match self.upcoming.take() {
            Some((date, task)) if date == session.date => Some(task.await??),
            _ => None,
        };
        let mut fetched = match upcoming.filter(|fetched| held.is_subset(&fetched.tracked)) {
            Some(fetched) => fetched,
            None => self.fetch(session.date, held.clone()).await?,
        };
        fetched.narrow(held);
        let previous = self
            .calendar
            .previous_before(session.date)
            .map_or(session.date, |previous| previous.date);
        self.session = SessionData {
            date: Some(session.date),
            universe: fetched
                .universe
                .iter()
                .filter(|symbol| !self.ended(symbol, session.date, session.date))
                .cloned()
                .collect(),
            universe_at: fetched.universe,
            actions: fetched.actions,
            liquidity: fetched.liquidity,
            delisted: fetched
                .tracked
                .iter()
                .filter(|symbol| self.ended(symbol, previous, session.date))
                .cloned()
                .collect(),
            risk_free_rate: fetched.risk_free_rate,
        };
        self.tracked = fetched.tracked;
        if self.prefetch_sessions
            && let Some(next) = self.calendar.next_after(session.date)
            && self.end.is_none_or(|end| next.date <= end)
        {
            let fetch = self.fetch(next.date, self.tracked.clone());
            self.upcoming = Some((next.date, tokio::spawn(fetch)));
        }
        Ok(())
    }

    fn fetch(
        &self,
        date: NaiveDate,
        held: BTreeSet<Symbol>,
    ) -> impl Future<Output = Result<Fetched, MarketError>> + Send + 'static {
        let market = self.market.clone();
        let universe = self.universe.clone();
        let adv_sessions = self.adv_sessions;
        let actions_from = self
            .calendar
            .previous_before(date)
            .and_then(|previous| previous.date.succ_opt())
            .unwrap_or(date);
        async move {
            let universe = market.universe(&universe, date, UNIVERSE_SESSIONS).await?;
            let tracked: BTreeSet<Symbol> = universe.iter().cloned().chain(held).collect();
            let symbols: Vec<Symbol> = tracked.iter().cloned().collect();
            let actions = market
                .traded_actions(symbols.clone(), actions_from, date)
                .await?;
            let liquidity = market.liquidity(symbols, date, adv_sessions).await?;
            let risk_free_rate = market
                .risk_free_rates(date - TimeDelta::days(RATE_LOOKBACK_DAYS), date)
                .await?
                .iter()
                .rfind(|rate| rate.date < date)
                .map(|rate| rate.percent / Decimal::ONE_HUNDRED);
            Ok(Fetched {
                universe,
                tracked,
                actions,
                liquidity,
                risk_free_rate,
            })
        }
    }

    pub(crate) async fn track(&mut self, held: BTreeSet<Symbol>) -> Result<(), MarketError> {
        let added: Vec<Symbol> = held
            .into_iter()
            .filter(|symbol| !self.tracked.contains(symbol))
            .collect();
        if let Some(date) = self.session.date
            && !added.is_empty()
        {
            let liquidity = self
                .market
                .liquidity(added.clone(), date, self.adv_sessions)
                .await?;
            self.session.liquidity.extend(liquidity);
        }
        self.tracked.extend(added);
        Ok(())
    }

    pub(crate) async fn session_so_far(&self, point: &Point) -> Result<Vec<Bar>, MarketError> {
        let through = point.visible_at - TimeDelta::seconds(60) + TIMESTAMP_RESOLUTION;
        let minutes = self
            .market
            .traded_bars(
                Timeframe::Minute,
                self.tracked.iter().cloned().collect(),
                point.session.open,
                through,
            )
            .await?;
        let mut so_far: BTreeMap<Symbol, Bar> = BTreeMap::new();
        for minute in minutes {
            match so_far.get_mut(&minute.symbol) {
                None => {
                    so_far.insert(
                        minute.symbol.clone(),
                        Bar {
                            start: point.session.open,
                            ..minute
                        },
                    );
                }
                Some(bar) => {
                    bar.high = bar.high.max(minute.high);
                    bar.low = bar.low.min(minute.low);
                    bar.close = minute.close;
                    bar.volume += minute.volume;
                    bar.feed = minute.feed;
                }
            }
        }
        Ok(so_far.into_values().collect())
    }

    pub(crate) fn prices_from_minutes(&self, session: &Session, at: Timestamp) -> bool {
        self.timeframe == Timeframe::Minute || !session.is_closed_at(at)
    }

    pub(crate) async fn last_closes(
        &self,
        session: &Session,
        at: Timestamp,
        symbols: Vec<Symbol>,
    ) -> Result<BTreeMap<Symbol, Decimal>, MarketError> {
        let mut closes = BTreeMap::new();
        if self.prices_from_minutes(session, at) {
            let through = at - TimeDelta::seconds(60) + TIMESTAMP_RESOLUTION;
            let minutes = self
                .market
                .traded_bars(Timeframe::Minute, symbols.clone(), session.open, through)
                .await?;
            closes.extend(minutes.into_iter().map(|bar| (bar.symbol, bar.close)));
        }
        let unpriced: Vec<Symbol> = symbols
            .into_iter()
            .filter(|symbol| !closes.contains_key(symbol))
            .collect();
        if !unpriced.is_empty() {
            closes.extend(self.market.split_adjusted_closes(unpriced, at).await?);
        }
        Ok(closes)
    }

    pub(crate) async fn official(
        &self,
        session: &Session,
        symbols: Vec<Symbol>,
    ) -> Result<Vec<Bar>, MarketError> {
        self.market
            .traded_bars(
                Timeframe::Day,
                symbols,
                session.open,
                session.open + TIMESTAMP_RESOLUTION,
            )
            .await
    }

    pub(crate) async fn bars(
        &mut self,
        point: &Point,
        visible_through: Option<Timestamp>,
    ) -> Result<Vec<Bar>, MarketError> {
        if !self.covers(&self.loaded, point) {
            let prefetched = match self.prefetch.take() {
                Some(task) => Some(task.await??),
                None => None,
            };
            self.loaded = match prefetched.filter(|loaded| self.covers(loaded, point)) {
                Some(loaded) => loaded,
                None => {
                    let to = self.chunk_end(&point.session, visible_through);
                    load(&self.market, self.timeframe, &self.tracked, point.start, to).await?
                }
            };
            if visible_through.is_none() {
                self.prefetch_after(self.loaded.to);
            }
        }
        let queue = &mut self.loaded.bars;
        while queue.front().is_some_and(|bar| bar.start < point.start) {
            queue.pop_front();
        }
        let current = queue
            .iter()
            .take_while(|bar| bar.start == point.start)
            .count();
        let tracked = &self.tracked;
        Ok(queue
            .drain(..current)
            .filter(|bar| tracked.contains(&bar.symbol))
            .collect())
    }

    fn covers(&self, loaded: &Loaded, point: &Point) -> bool {
        loaded.from <= point.start
            && point.start < loaded.to
            && self.tracked.is_subset(&loaded.symbols)
    }

    fn prefetch_after(&mut self, from: Timestamp) {
        let sessions = self.calendar.sessions();
        let Some(next) = sessions.get(sessions.partition_point(|session| session.open < from))
        else {
            return;
        };
        if self.end.is_some_and(|end| next.date > end) {
            return;
        }
        let to = self.chunk_end(next, None);
        let (market, timeframe, tracked) =
            (self.market.clone(), self.timeframe, self.tracked.clone());
        self.prefetch = Some(tokio::spawn(async move {
            load(&market, timeframe, &tracked, from, to).await
        }));
    }

    fn chunk_end(&self, session: &Session, visible_through: Option<Timestamp>) -> Timestamp {
        let sessions = self.calendar.sessions();
        let first = sessions.partition_point(|candidate| candidate.date < session.date);
        let chunk = match self.timeframe {
            Timeframe::Day => 250,
            Timeframe::Minute => 2,
        };
        let mut last = (first + chunk - 1).min(sessions.len().saturating_sub(1));
        if let Some(end) = self.end {
            last = last.min(
                sessions
                    .partition_point(|candidate| candidate.date <= end)
                    .saturating_sub(1),
            );
        }
        let mut to = sessions
            .get(last.max(first))
            .map_or(session.close, |last| last.close);
        if let Some(through) = visible_through {
            let bound = match self.timeframe {
                Timeframe::Minute => through - TimeDelta::seconds(60) + TIMESTAMP_RESOLUTION,
                Timeframe::Day => sessions
                    .get(sessions.partition_point(|candidate| candidate.close <= through))
                    .map_or(Timestamp::MAX, |candidate| candidate.open),
            };
            to = to.min(bound);
        }
        to
    }
}

async fn load(
    market: &MarketStore,
    timeframe: Timeframe,
    tracked: &BTreeSet<Symbol>,
    from: Timestamp,
    to: Timestamp,
) -> Result<Loaded, MarketError> {
    let bars = market
        .traded_bars(timeframe, tracked.iter().cloned().collect(), from, to)
        .await?;
    Ok(Loaded {
        symbols: tracked.clone(),
        from,
        to,
        bars: bars.into(),
    })
}
