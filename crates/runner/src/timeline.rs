use std::collections::{BTreeMap, BTreeSet, VecDeque};

use buizz_core::market::{Bar, Calendar, CorporateAction, Instrument, Session, Symbol, Timeframe};
use buizz_core::run::{RunConfig, UniverseSpec};
use buizz_core::time::Timestamp;
use buizz_core::visibility::bar_visible_at;
use buizz_market::{MarketError, MarketStore};
use chrono::{NaiveDate, TimeDelta};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Point {
    pub session: Session,
    pub start: Timestamp,
    pub visible_at: Timestamp,
}

#[derive(Debug, Default)]
pub(crate) struct SessionData {
    pub date: Option<NaiveDate>,
    pub universe: Vec<Symbol>,
    pub actions: Vec<CorporateAction>,
    pub average_daily_volume: BTreeMap<Symbol, u64>,
    pub delisted: Vec<Symbol>,
}

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
    universe: UniverseSpec,
    adv_sessions: u32,
    start: NaiveDate,
    end: Option<NaiveDate>,
    calendar: Calendar,
    instruments: BTreeMap<Symbol, Instrument>,
    session: SessionData,
    loaded: Loaded,
}

impl Timeline {
    pub(crate) async fn open(market: MarketStore, config: &RunConfig) -> Result<Self, MarketError> {
        let mut timeline = Self {
            market,
            timeframe: config.timeframe,
            universe: config.universe.clone(),
            adv_sessions: config.cost.adv_sessions,
            start: config.start,
            end: config.end,
            calendar: Calendar::default(),
            instruments: BTreeMap::new(),
            session: SessionData::default(),
            loaded: Loaded::default(),
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
        Ok(())
    }

    pub(crate) fn calendar(&self) -> &Calendar {
        &self.calendar
    }

    pub(crate) fn instrument(&self, symbol: &Symbol) -> Option<&Instrument> {
        self.instruments.get(symbol)
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
        })
    }

    pub(crate) async fn enter(
        &mut self,
        session: &Session,
        held: &BTreeSet<Symbol>,
    ) -> Result<(), MarketError> {
        if self.session.date == Some(session.date) {
            return Ok(());
        }
        let listed = |symbol: &Symbol| {
            self.instruments
                .get(symbol)
                .and_then(|instrument| instrument.last_session)
                .is_none_or(|last| last >= session.date)
        };
        let universe: Vec<Symbol> = match &self.universe {
            UniverseSpec::Symbols { symbols } => symbols.clone(),
            UniverseSpec::TopDollarVolume { top_dollar_volume } => {
                self.market
                    .top_dollar_volume(*top_dollar_volume, session.date, self.adv_sessions)
                    .await?
            }
        };
        let universe: Vec<Symbol> = universe
            .into_iter()
            .filter(|symbol| listed(symbol))
            .collect();
        let tracked: Vec<Symbol> = universe
            .iter()
            .chain(held)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let delisted = tracked
            .iter()
            .filter(|symbol| !listed(symbol))
            .cloned()
            .collect();
        let actions_from = self
            .calendar
            .previous_before(session.date)
            .and_then(|previous| previous.date.succ_opt())
            .unwrap_or(session.date);
        let actions = self
            .market
            .corporate_actions(tracked.clone(), actions_from, session.date)
            .await?;
        let average_daily_volume = self
            .market
            .average_daily_volume(tracked, session.date, self.adv_sessions)
            .await?;
        self.session = SessionData {
            date: Some(session.date),
            universe,
            actions,
            average_daily_volume,
            delisted,
        };
        Ok(())
    }

    pub(crate) fn tracked(&self, held: &BTreeSet<Symbol>) -> BTreeSet<Symbol> {
        self.session.universe.iter().chain(held).cloned().collect()
    }

    pub(crate) async fn bars(
        &mut self,
        point: &Point,
        tracked: &BTreeSet<Symbol>,
        visible_through: Option<Timestamp>,
    ) -> Result<Vec<Bar>, MarketError> {
        let loaded = &self.loaded;
        let covers = loaded.from <= point.start
            && point.start < loaded.to
            && tracked.is_subset(&loaded.symbols);
        if !covers {
            let to = self.chunk_end(point, visible_through);
            let bars = self
                .market
                .bars_between(
                    self.timeframe,
                    tracked.iter().cloned().collect(),
                    point.start,
                    to,
                )
                .await?;
            self.loaded = Loaded {
                symbols: tracked.clone(),
                from: point.start,
                to,
                bars: bars.into(),
            };
        }
        let queue = &mut self.loaded.bars;
        while queue.front().is_some_and(|bar| bar.start < point.start) {
            queue.pop_front();
        }
        let current = queue
            .iter()
            .take_while(|bar| bar.start == point.start)
            .count();
        let bars = queue
            .drain(..current)
            .filter(|bar| tracked.contains(&bar.symbol))
            .collect();
        Ok(bars)
    }

    fn chunk_end(&self, point: &Point, visible_through: Option<Timestamp>) -> Timestamp {
        let sessions = self.calendar.sessions();
        let first = sessions.partition_point(|session| session.date < point.session.date);
        let chunk = match self.timeframe {
            Timeframe::Day => 250,
            Timeframe::Minute => 2,
        };
        let mut last = (first + chunk - 1).min(sessions.len().saturating_sub(1));
        if let Some(end) = self.end {
            last = last.min(
                sessions
                    .partition_point(|session| session.date <= end)
                    .saturating_sub(1),
            );
        }
        let mut to = sessions
            .get(last.max(first))
            .map_or(point.session.close, |session| session.close);
        if let Some(through) = visible_through {
            let bound = match self.timeframe {
                Timeframe::Minute => through - TimeDelta::seconds(60) + TimeDelta::nanoseconds(1),
                Timeframe::Day => sessions
                    .get(sessions.partition_point(|session| session.close <= through))
                    .map_or(Timestamp::MAX, |session| session.open),
            };
            to = to.min(bound);
        }
        to
    }
}
