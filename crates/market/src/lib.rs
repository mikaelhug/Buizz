use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, PoisonError, RwLock};

use buizz_core::market::{
    Adjustment, Bar, Calendar, CorporateAction, Coverage, DataSource, FundamentalFact,
    FundamentalMetric, FundamentalPoint, Instrument, MacroObservation, MacroPoint, MacroSeries,
    RiskFreeRate, Session, Symbol, Timeframe, Watermarks,
};
use buizz_core::ops::{CoverageGap, DataWarning};
use buizz_core::protocol::BarRow;
use buizz_core::time::Timestamp;
use buizz_core::visibility::{bar_visible_at, published_sessions, universe_visible_at};
use chrono::{NaiveDate, TimeDelta};
use duckdb::Connection;
use tokio::sync::watch;

use crate::fixture::{ActionRow, DailyRow};
use crate::lane::Lane;
use crate::read::BarQuery;

mod codec;
mod fixture;
mod lane;
mod read;
mod write;

pub const SCHEMA: &str = include_str!("../schema.sql");
pub const FILE_NAME: &str = "market.duckdb";
pub const TIMESTAMP_RESOLUTION: TimeDelta = TimeDelta::microseconds(1);

#[derive(Debug, thiserror::Error)]
pub enum MarketError {
    #[error("market store: {0}")]
    Db(#[from] duckdb::Error),
    #[error("market store fixture: {0}")]
    Fixture(String),
    #[error("market store io: {0}")]
    Io(#[from] std::io::Error),
    #[error("market store has shut down")]
    Closed,
    #[error("market store worker failed: {0}")]
    Worker(#[from] tokio::task::JoinError),
    #[error("market store job panicked: {0}")]
    Panicked(String),
}

pub type Result<T> = std::result::Result<T, MarketError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRequest {
    pub symbols: Vec<Symbol>,
    pub timeframe: Timeframe,
    pub bars: u32,
    pub adjust: Adjustment,
    pub as_of: Timestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteSummary {
    pub batch_id: i64,
    pub rows: u64,
}

struct Shared {
    calendar: RwLock<Arc<Calendar>>,
    watermarks: watch::Sender<Watermarks>,
}

impl Shared {
    fn calendar(&self) -> Arc<Calendar> {
        Arc::clone(&self.calendar.read().unwrap_or_else(PoisonError::into_inner))
    }

    fn refresh_calendar(&self, connection: &Connection) -> Result<()> {
        let calendar = Arc::new(read::calendar(connection)?);
        *self
            .calendar
            .write()
            .unwrap_or_else(PoisonError::into_inner) = calendar;
        Ok(())
    }
}

#[derive(Clone)]
pub struct MarketStore {
    writer: Lane,
    readers: Lane,
    shared: Arc<Shared>,
}

impl MarketStore {
    pub async fn open(path: &Path, readers: usize) -> Result<Self> {
        let path = path.to_owned();
        let (writer, pool, calendar) = tokio::task::spawn_blocking(move || {
            let writer = Connection::open(path)?;
            writer.execute_batch(SCHEMA)?;
            write::prepare(&writer)?;
            let pool = (0..readers.max(1))
                .map(|_| writer.try_clone())
                .collect::<duckdb::Result<Vec<_>>>()?;
            let calendar = read::calendar(&writer)?;
            Ok::<_, MarketError>((writer, pool, calendar))
        })
        .await??;
        Ok(Self {
            writer: Lane::spawn("market-writer", vec![writer])?,
            readers: Lane::spawn("market-reader", pool)?,
            shared: Arc::new(Shared {
                calendar: RwLock::new(Arc::new(calendar)),
                watermarks: watch::Sender::new(Watermarks::default()),
            }),
        })
    }

    pub async fn load_fixture(&self, dir: &Path) -> Result<()> {
        let source = DataSource::Fixture;
        self.upsert_sessions(source, fixture::rows(dir, "calendar.csv").await?)
            .await?;
        self.upsert_instruments(source, fixture::rows(dir, "instruments.csv").await?)
            .await?;
        let calendar = self.shared.calendar();
        let daily = fixture::rows::<DailyRow>(dir, "bar_1d.csv")
            .await?
            .into_iter()
            .map(|row| row.into_bar(&calendar))
            .collect::<Result<Vec<_>>>()?;
        let minute: Vec<Bar> = fixture::rows(dir, "bar_1m.csv").await?;
        let loaded = [
            (
                Timeframe::Day,
                visible_through(Timeframe::Day, &daily, &calendar),
            ),
            (
                Timeframe::Minute,
                visible_through(Timeframe::Minute, &minute, &calendar),
            ),
        ];
        self.upsert_bars(source, Timeframe::Day, daily).await?;
        self.upsert_bars(source, Timeframe::Minute, minute).await?;
        let actions = fixture::rows::<ActionRow>(dir, "corporate_actions.csv")
            .await?
            .into_iter()
            .map(ActionRow::into_action)
            .collect::<Result<Vec<_>>>()?;
        let covered: Vec<Symbol> = actions
            .iter()
            .map(|action| action.symbol.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let ex_dates = actions.iter().map(|action| action.ex_date);
        if let (Some(from), Some(to)) = (ex_dates.clone().min(), ex_dates.max()) {
            self.upsert_corporate_actions(source, actions, covered, from, to)
                .await?;
        }
        let facts = fixture::rows(dir, "fundamental_facts.csv").await?;
        self.append_fundamental_facts(source, facts).await?;
        self.upsert_risk_free_rates(source, fixture::rows(dir, "rates.csv").await?)
            .await?;
        self.upsert_macro_observations(source, fixture::rows(dir, "macro.csv").await?)
            .await?;
        for (timeframe, through) in loaded {
            if let Some(through) = through {
                self.advance_watermark(timeframe, through).await?;
            }
        }
        Ok(())
    }

    pub async fn upsert_instruments(
        &self,
        source: DataSource,
        instruments: Vec<Instrument>,
    ) -> Result<WriteSummary> {
        self.writer
            .run(move |connection| write::instruments(connection, source, instruments))
            .await
    }

    pub async fn upsert_sessions(
        &self,
        source: DataSource,
        sessions: Vec<Session>,
    ) -> Result<WriteSummary> {
        self.write_sessions(source, None, sessions).await
    }

    pub async fn replace_sessions(
        &self,
        source: DataSource,
        from: NaiveDate,
        to: NaiveDate,
        sessions: Vec<Session>,
    ) -> Result<WriteSummary> {
        self.write_sessions(source, Some((from, to)), sessions)
            .await
    }

    async fn write_sessions(
        &self,
        source: DataSource,
        replaced: Option<(NaiveDate, NaiveDate)>,
        sessions: Vec<Session>,
    ) -> Result<WriteSummary> {
        let shared = Arc::clone(&self.shared);
        self.writer
            .run(move |connection| {
                let summary = write::sessions(connection, source, replaced, sessions)?;
                shared.refresh_calendar(connection)?;
                Ok(summary)
            })
            .await
    }

    pub async fn upsert_bars(
        &self,
        source: DataSource,
        timeframe: Timeframe,
        bars: Vec<Bar>,
    ) -> Result<WriteSummary> {
        self.writer
            .run(move |connection| match timeframe {
                Timeframe::Day => write::daily_bars(connection, source, bars),
                Timeframe::Minute => write::minute_bars(connection, source, bars),
            })
            .await
    }

    pub async fn upsert_corporate_actions(
        &self,
        source: DataSource,
        actions: Vec<CorporateAction>,
        covered: Vec<Symbol>,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<WriteSummary> {
        self.writer
            .run(move |connection| {
                write::corporate_actions(connection, source, actions, &covered, (from, to))
            })
            .await
    }

    pub async fn append_fundamental_facts(
        &self,
        source: DataSource,
        facts: Vec<FundamentalFact>,
    ) -> Result<WriteSummary> {
        self.writer
            .run(move |connection| write::fundamental_facts(connection, source, facts))
            .await
    }

    pub async fn upsert_risk_free_rates(
        &self,
        source: DataSource,
        rates: Vec<RiskFreeRate>,
    ) -> Result<WriteSummary> {
        self.writer
            .run(move |connection| write::risk_free_rates(connection, source, rates))
            .await
    }

    pub async fn upsert_macro_observations(
        &self,
        source: DataSource,
        observations: Vec<MacroObservation>,
    ) -> Result<WriteSummary> {
        self.writer
            .run(move |connection| write::macro_observations(connection, source, observations))
            .await
    }

    pub async fn advance_watermark(&self, timeframe: Timeframe, through: Timestamp) -> Result<()> {
        let shared = Arc::clone(&self.shared);
        self.writer
            .run(move |_| {
                shared.watermarks.send_if_modified(|watermarks| {
                    let slot = match timeframe {
                        Timeframe::Day => &mut watermarks.day,
                        Timeframe::Minute => &mut watermarks.minute,
                    };
                    let advanced = slot.is_none_or(|current| current < through);
                    if advanced {
                        *slot = Some(through);
                    }
                    advanced
                });
                Ok(())
            })
            .await
    }

    pub fn watermarks(&self) -> watch::Receiver<Watermarks> {
        self.shared.watermarks.subscribe()
    }

    pub async fn calendar(&self, from: NaiveDate, to: NaiveDate) -> Result<Calendar> {
        Ok(Calendar::new(
            self.shared.calendar().between(from, to).to_vec(),
        ))
    }

    pub fn published_sessions(
        &self,
        from: NaiveDate,
        to: NaiveDate,
        as_of: Timestamp,
    ) -> Vec<Session> {
        published_sessions(&self.shared.calendar(), from, to, as_of)
    }

    pub fn universe_visible_at(&self, date: NaiveDate) -> Timestamp {
        universe_visible_at(date, &self.shared.calendar())
    }

    pub async fn instruments(&self) -> Result<Vec<Instrument>> {
        self.readers
            .run(|connection| read::instruments(connection))
            .await
    }

    pub async fn bars_between(
        &self,
        timeframe: Timeframe,
        symbols: Vec<Symbol>,
        from: Timestamp,
        to: Timestamp,
    ) -> Result<Vec<Bar>> {
        self.readers
            .run(move |connection| {
                read::bars(
                    connection,
                    &BarQuery {
                        timeframe,
                        symbols: &symbols,
                        from: Some(from),
                        to,
                        to_inclusive: false,
                        latest: None,
                    },
                )
            })
            .await
    }

    pub async fn history(&self, request: HistoryRequest) -> Result<Vec<BarRow>> {
        let calendar = self.shared.calendar();
        self.readers
            .run(move |connection| read::history(connection, &calendar, &request))
            .await
    }

    pub async fn fundamentals(
        &self,
        symbols: Vec<Symbol>,
        metrics: Vec<FundamentalMetric>,
        as_of: Timestamp,
    ) -> Result<Vec<FundamentalPoint>> {
        self.readers
            .run(move |connection| read::fundamentals(connection, &symbols, &metrics, as_of))
            .await
    }

    pub async fn corporate_actions(
        &self,
        symbols: Vec<Symbol>,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<CorporateAction>> {
        self.readers
            .run(move |connection| read::corporate_actions(connection, &symbols, from, to))
            .await
    }

    pub async fn average_daily_volume(
        &self,
        symbols: Vec<Symbol>,
        before: NaiveDate,
        sessions: u32,
    ) -> Result<BTreeMap<Symbol, u64>> {
        let calendar = self.shared.calendar();
        self.readers
            .run(move |connection| {
                read::average_daily_volume(connection, &calendar, &symbols, before, sessions)
            })
            .await
    }

    pub async fn top_dollar_volume(
        &self,
        count: u32,
        before: NaiveDate,
        sessions: u32,
    ) -> Result<Vec<Symbol>> {
        let calendar = self.shared.calendar();
        self.readers
            .run(move |connection| {
                read::top_dollar_volume(connection, &calendar, count, before, sessions)
            })
            .await
    }

    pub async fn listed_top_dollar_volume(
        &self,
        count: u32,
        date: NaiveDate,
        sessions: u32,
    ) -> Result<Vec<Symbol>> {
        let calendar = self.shared.calendar();
        self.readers
            .run(move |connection| {
                let delisted: BTreeSet<Symbol> = read::instruments(connection)?
                    .into_iter()
                    .filter(|instrument| instrument.last_session.is_some_and(|last| last < date))
                    .map(|instrument| instrument.symbol)
                    .collect();
                let mut symbols =
                    read::top_dollar_volume(connection, &calendar, count, date, sessions)?;
                symbols.retain(|symbol| !delisted.contains(symbol));
                Ok(symbols)
            })
            .await
    }

    pub async fn risk_free_rates(
        &self,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<RiskFreeRate>> {
        self.readers
            .run(move |connection| read::risk_free_rates(connection, from, to))
            .await
    }

    pub async fn macro_observations(&self, series: MacroSeries) -> Result<Vec<MacroObservation>> {
        self.readers
            .run(move |connection| read::macro_observations(connection, series))
            .await
    }

    pub async fn macro_points(
        &self,
        series: Vec<MacroSeries>,
        from: NaiveDate,
        to: NaiveDate,
        as_of: Timestamp,
    ) -> Result<Vec<MacroPoint>> {
        let calendar = self.shared.calendar();
        self.readers
            .run(move |connection| {
                read::macro_points(connection, &calendar, &series, from, to, as_of)
            })
            .await
    }

    pub async fn coverage(&self) -> Result<Vec<Coverage>> {
        self.readers
            .run(|connection| read::coverage(connection))
            .await
    }

    pub async fn coverage_gaps(
        &self,
        symbols: Vec<Symbol>,
        timeframe: Timeframe,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<CoverageGap>> {
        let calendar = self.shared.calendar();
        self.readers
            .run(move |connection| {
                read::coverage_gaps(connection, &calendar, &symbols, timeframe, from, to)
            })
            .await
    }

    pub async fn unexplained_moves(
        &self,
        symbols: Vec<Symbol>,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<DataWarning>> {
        let calendar = self.shared.calendar();
        self.readers
            .run(move |connection| {
                read::unexplained_moves(connection, &calendar, &symbols, from, to)
            })
            .await
    }
}

fn visible_through(timeframe: Timeframe, bars: &[Bar], calendar: &Calendar) -> Option<Timestamp> {
    bars.iter()
        .filter_map(|bar| {
            let session = calendar.containing(bar.start)?;
            Some(bar_visible_at(timeframe, bar.start, session))
        })
        .max()
}
