use std::collections::{BTreeMap, BTreeSet};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};

use buizz_core::listing::Listings;
use buizz_core::market::{
    Adjustment, AdjustmentFactor, Bar, BorrowAvailability, Calendar, Company, CorporateAction,
    Coverage, DataSource, EarningsInsight, EarningsRelease, EstimateFact, FomcAnnouncement,
    FomcMeeting, FundamentalFact, FundamentalMetric, FundamentalPoint, IndexHistory, InsiderFiling,
    InsiderTrade, Instrument, InstrumentProfile, MacroObservation, MacroPoint, MacroSeries,
    NewsArticle, RiskFreeRate, SecFiling, Session, StockIndex, Symbol, Timeframe, Watermarks,
};
use buizz_core::ops::{ActionDisagreement, CoverageGap, DataWarning};
use buizz_core::protocol::BarRow;
use buizz_core::run::UniverseSpec;
use buizz_core::time::Timestamp;
use buizz_core::visibility::{bar_visible_at, published_sessions, universe_visible_at};
use chrono::{NaiveDate, TimeDelta};
use duckdb::Connection;
use rust_decimal::Decimal;
use tokio::sync::watch;

use crate::fixture::{ActionRow, DailyRow};
use crate::lane::Lane;
use crate::read::{Actions, BarQuery};

mod codec;
mod fixture;
mod lane;
mod migrate;
mod read;
mod restore;
mod write;

pub use restore::Restored;

pub(crate) const SCHEMA: &str = include_str!("../schema.sql");
pub const FILE_NAME: &str = "market.duckdb";
const APPENDED: [(&str, &str); 17] = [
    ("ingest_batch", "id"),
    ("borrow_availability", "batch_id"),
    ("news_article", "batch_id"),
    ("instrument_revision", "superseded_by"),
    ("calendar_revision", "superseded_by"),
    ("bar_1d_revision", "superseded_by"),
    ("corporate_action_revision", "superseded_by"),
    ("corporate_action_check_revision", "superseded_by"),
    ("rate_daily_revision", "superseded_by"),
    ("macro_observation_revision", "superseded_by"),
    ("fomc_meeting_revision", "superseded_by"),
    ("company_revision", "superseded_by"),
    ("earnings_release_revision", "superseded_by"),
    ("index_member_revision", "superseded_by"),
    ("index_history_revision", "superseded_by"),
    ("earnings_insight", "batch_id"),
    ("earnings_insight_fact_revision", "superseded_by"),
];
const REPLACED: [&str; 2] = ["index_member", "index_history"];
pub const TIMESTAMP_RESOLUTION: TimeDelta = TimeDelta::microseconds(1);

#[derive(Debug, thiserror::Error)]
pub enum MarketError {
    #[error("market store: {0}")]
    Db(#[from] duckdb::Error),
    #[error("market store fixture: {0}")]
    Fixture(String),
    #[error("market store io: {0}")]
    Io(#[from] std::io::Error),
    #[error("market store schema is version {found}, newer than the {known} this spine knows")]
    NewerSchema { found: i64, known: i64 },
    #[error("market restore: {0}")]
    Restore(String),
    #[error("market store setting: {0}")]
    Setting(String),
    #[error("market store catalog: {0}")]
    Catalog(String),
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChartRequest {
    pub symbol: Symbol,
    pub timeframe: Timeframe,
    pub from: Timestamp,
    pub after: Option<Timestamp>,
    pub before: Option<Timestamp>,
    pub bars: usize,
    pub as_of: Timestamp,
    pub listed_on: NaiveDate,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChartBars {
    pub bars: Vec<BarRow>,
    pub older: bool,
    pub splits: Vec<AdjustmentFactor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionWindow {
    pub symbols: Vec<Symbol>,
    pub from: NaiveDate,
    pub through: NaiveDate,
    pub covered_to: Option<NaiveDate>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Export {
    pub through: i64,
    pub rows: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteSummary {
    pub batch_id: i64,
    pub rows: u64,
}

struct Shared {
    calendar: RwLock<Arc<Calendar>>,
    listings: RwLock<Arc<Listings>>,
    actions: RwLock<Arc<Actions>>,
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

    fn listings(&self) -> Arc<Listings> {
        Arc::clone(&self.listings.read().unwrap_or_else(PoisonError::into_inner))
    }

    fn refresh_listings(&self, connection: &Connection) -> Result<()> {
        let listings = Arc::new(read::listings(connection)?);
        *self
            .listings
            .write()
            .unwrap_or_else(PoisonError::into_inner) = listings;
        Ok(())
    }

    fn actions(&self) -> Arc<Actions> {
        Arc::clone(&self.actions.read().unwrap_or_else(PoisonError::into_inner))
    }

    fn refresh_actions(&self, connection: &Connection) -> Result<()> {
        let actions = Arc::new(read::actions(connection)?);
        *self.actions.write().unwrap_or_else(PoisonError::into_inner) = actions;
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
        let (writer, pool, calendar, listings, actions, day) =
            tokio::task::spawn_blocking(move || {
                let mut writer = Connection::open(path)?;
                migrate::migrate(&mut writer)?;
                write::prepare(&writer)?;
                let pool = (0..readers.max(1))
                    .map(|_| writer.try_clone())
                    .collect::<duckdb::Result<Vec<_>>>()?;
                let calendar = read::calendar(&writer)?;
                let listings = read::listings(&writer)?;
                let actions = read::actions(&writer)?;
                let day = read::daily_watermark(&writer)?;
                Ok::<_, MarketError>((writer, pool, calendar, listings, actions, day))
            })
            .await??;
        Ok(Self {
            writer: Lane::spawn("market-writer", vec![writer])?,
            readers: Lane::spawn("market-reader", pool)?,
            shared: Arc::new(Shared {
                calendar: RwLock::new(Arc::new(calendar)),
                listings: RwLock::new(Arc::new(listings)),
                actions: RwLock::new(Arc::new(actions)),
                watermarks: watch::Sender::new(Watermarks { day, minute: None }),
            }),
        })
    }

    pub async fn limit(&self, memory: String, threads: usize) -> Result<()> {
        if memory.is_empty()
            || !memory
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.')
        {
            return Err(MarketError::Setting(format!("memory limit {memory:?}")));
        }
        self.writer
            .run(move |connection| {
                connection.execute_batch(&format!(
                    "SET memory_limit = '{memory}'; SET threads = {}",
                    threads.max(1)
                ))?;
                Ok(())
            })
            .await
    }

    pub async fn export_unrecoverable(
        &self,
        directory: PathBuf,
        after: i64,
    ) -> Result<Option<Export>> {
        self.readers
            .run(move |connection| {
                let transaction = connection.transaction()?;
                let through: i64 = transaction.query_row(
                    "SELECT coalesce(max(id), 0) FROM ingest_batch",
                    [],
                    |row| row.get(0),
                )?;
                if through <= after {
                    return Ok(None);
                }
                let copy = |query: String, table: &str| -> Result<u64> {
                    let path = directory.join(format!("{table}.parquet"));
                    let path = path.to_string_lossy().replace('\'', "''");
                    Ok(transaction
                        .execute(&format!("COPY ({query}) TO '{path}' (FORMAT parquet)"), [])?
                        as u64)
                };
                let mut rows = 0;
                for (table, batch) in APPENDED {
                    rows += copy(
                        format!(
                            "SELECT * FROM {table} WHERE {batch} > {after} AND {batch} <= {through}"
                        ),
                        table,
                    )?;
                }
                let index_changed: bool = transaction.query_row(
                    "SELECT coalesce(max(batch_id), 0) > ? FROM index_history",
                    [after],
                    |row| row.get(0),
                )?;
                if index_changed {
                    for table in REPLACED {
                        rows += copy(format!("SELECT * FROM {table}"), table)?;
                    }
                }
                Ok(Some(Export { through, rows }))
            })
            .await
    }

    pub async fn restore(&self, backup: PathBuf) -> Result<Restored> {
        let shared = Arc::clone(&self.shared);
        self.writer
            .run(move |connection| {
                let parts = restore::parts(&backup)?;
                let restored = restore::restore(connection, &parts)?;
                shared.refresh_listings(connection)?;
                shared.refresh_actions(connection)?;
                Ok(restored)
            })
            .await
    }

    pub async fn is_empty(&self) -> Result<bool> {
        self.readers
            .run(|connection| {
                Ok(
                    connection.query_row("SELECT count(*) = 0 FROM ingest_batch", [], |row| {
                        row.get(0)
                    })?,
                )
            })
            .await
    }

    pub async fn checkpoint(&self) -> Result<()> {
        self.writer
            .run(|connection| Ok(connection.execute_batch("CHECKPOINT")?))
            .await
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
            let window = ActionWindow {
                symbols: covered,
                from,
                through: to,
                covered_to: Some(to),
            };
            self.replace_corporate_actions(source, actions, window)
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
        let shared = Arc::clone(&self.shared);
        self.writer
            .run(move |connection| {
                let summary = write::instruments(connection, source, instruments)?;
                shared.refresh_listings(connection)?;
                Ok(summary)
            })
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
                Timeframe::Day => {
                    let summary = write::daily_bars(connection, source, bars)?;
                    if let Err(error) = keep_daily_bars_ordered(connection) {
                        tracing::warn!(%error, "cannot put the daily bars back in date order");
                    }
                    Ok(summary)
                }
                Timeframe::Minute => write::minute_bars(connection, source, bars),
            })
            .await
    }

    pub async fn record_quiet_sessions(
        &self,
        source: DataSource,
        sessions: Vec<(Symbol, NaiveDate)>,
    ) -> Result<WriteSummary> {
        self.writer
            .run(move |connection| write::quiet_sessions(connection, source, sessions))
            .await
    }

    pub async fn end_quiet_sessions(
        &self,
        source: DataSource,
        ticker: Symbol,
        ended: Symbol,
        last: NaiveDate,
    ) -> Result<WriteSummary> {
        self.writer
            .run(move |connection| {
                write::ended_quiet_sessions(connection, source, ticker, ended, last)
            })
            .await
    }

    pub async fn order_daily_bars(&self) -> Result<()> {
        self.writer.run(keep_daily_bars_ordered).await
    }

    pub async fn replace_corporate_actions(
        &self,
        source: DataSource,
        actions: Vec<CorporateAction>,
        window: ActionWindow,
    ) -> Result<WriteSummary> {
        let shared = Arc::clone(&self.shared);
        self.writer
            .run(move |connection| {
                let summary = write::corporate_actions(connection, source, actions, &window, None)?;
                shared.refresh_actions(connection)?;
                Ok(summary)
            })
            .await
    }

    pub async fn replace_name_changes(
        &self,
        source: DataSource,
        actions: Vec<CorporateAction>,
        window: ActionWindow,
    ) -> Result<WriteSummary> {
        let shared = Arc::clone(&self.shared);
        self.writer
            .run(move |connection| {
                let summary = write::corporate_actions(
                    connection,
                    source,
                    actions,
                    &window,
                    Some("c.kind = 'name_change'"),
                )?;
                shared.refresh_listings(connection)?;
                shared.refresh_actions(connection)?;
                Ok(summary)
            })
            .await
    }

    pub async fn replace_stock_mergers(
        &self,
        source: DataSource,
        actions: Vec<CorporateAction>,
        window: ActionWindow,
    ) -> Result<WriteSummary> {
        let shared = Arc::clone(&self.shared);
        self.writer
            .run(move |connection| {
                let summary = write::corporate_actions(
                    connection,
                    source,
                    actions,
                    &window,
                    Some(
                        "c.kind = 'stock_merger' AND c.old_rate = c.new_rate \
                         AND coalesce(c.rate, 0) = 0",
                    ),
                )?;
                shared.refresh_listings(connection)?;
                shared.refresh_actions(connection)?;
                Ok(summary)
            })
            .await
    }

    pub async fn withdraw_shadowed_bars(
        &self,
        source: DataSource,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<WriteSummary> {
        let shadowed = self.shared.listings().shadowed(from, to);
        self.writer
            .run(move |connection| write::shadowed_bars(connection, source, shadowed))
            .await
    }

    pub fn listings(&self) -> Arc<Listings> {
        self.shared.listings()
    }

    pub async fn actions_reaching_other_symbols(&self) -> Result<Vec<CorporateAction>> {
        self.readers
            .run(|connection| read::actions_reaching_other_symbols(connection))
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

    pub async fn replace_fomc_meetings(
        &self,
        source: DataSource,
        years: RangeInclusive<i32>,
        meetings: Vec<FomcMeeting>,
    ) -> Result<WriteSummary> {
        self.writer
            .run(move |connection| write::fomc_meetings(connection, source, years, meetings))
            .await
    }

    pub async fn append_earnings_insight(
        &self,
        source: DataSource,
        report: EarningsInsight,
    ) -> Result<WriteSummary> {
        self.writer
            .run(move |connection| write::earnings_insight(connection, source, report))
            .await
    }

    pub async fn replace_earnings_insight_facts(
        &self,
        source: DataSource,
        facts: BTreeMap<String, Vec<EstimateFact>>,
    ) -> Result<WriteSummary> {
        self.writer
            .run(move |connection| write::earnings_insight_facts(connection, source, facts))
            .await
    }

    pub async fn earnings_insight_reports(&self) -> Result<BTreeSet<String>> {
        self.readers
            .run(|connection| read::earnings_insight_reports(connection))
            .await
    }

    pub async fn earnings_insights(&self) -> Result<Vec<EarningsInsight>> {
        self.readers
            .run(|connection| read::earnings_insights(connection))
            .await
    }

    pub async fn earnings_insight_facts(&self) -> Result<BTreeMap<String, Vec<EstimateFact>>> {
        self.readers
            .run(|connection| read::earnings_insight_facts(connection))
            .await
    }

    pub async fn upsert_companies(
        &self,
        source: DataSource,
        companies: Vec<Company>,
    ) -> Result<WriteSummary> {
        self.writer
            .run(move |connection| write::companies(connection, source, companies))
            .await
    }

    pub async fn upsert_earnings_releases(
        &self,
        source: DataSource,
        releases: Vec<EarningsRelease>,
    ) -> Result<WriteSummary> {
        self.writer
            .run(move |connection| write::earnings_releases(connection, source, releases))
            .await
    }

    pub async fn append_insider_filings(
        &self,
        source: DataSource,
        filings: Vec<InsiderFiling>,
    ) -> Result<WriteSummary> {
        self.writer
            .run(move |connection| write::insider_filings(connection, source, filings))
            .await
    }

    pub async fn append_news(
        &self,
        source: DataSource,
        articles: Vec<NewsArticle>,
        window: Option<(NaiveDate, NaiveDate)>,
    ) -> Result<WriteSummary> {
        self.writer
            .run(move |connection| write::news_articles(connection, source, articles, window))
            .await
    }

    pub async fn replace_index_history(
        &self,
        source: DataSource,
        history: IndexHistory,
        today: NaiveDate,
    ) -> Result<WriteSummary> {
        self.writer
            .run(move |connection| write::index_history(connection, source, history, today))
            .await
    }

    pub async fn append_sec_filings(
        &self,
        source: DataSource,
        filings: Vec<SecFiling>,
        day: NaiveDate,
    ) -> Result<WriteSummary> {
        self.writer
            .run(move |connection| write::sec_filings(connection, source, filings, day))
            .await
    }

    pub async fn latest_sec_filings(
        &self,
        forms: &'static [&'static str],
    ) -> Result<BTreeMap<u32, NaiveDate>> {
        self.readers
            .run(move |connection| read::latest_sec_filings(connection, forms))
            .await
    }

    pub async fn append_borrow_availability(
        &self,
        source: DataSource,
        records: Vec<BorrowAvailability>,
    ) -> Result<WriteSummary> {
        self.writer
            .run(move |connection| write::borrow_availability(connection, source, records))
            .await
    }

    pub async fn advance_watermark(&self, timeframe: Timeframe, through: Timestamp) -> Result<()> {
        let shared = Arc::clone(&self.shared);
        self.writer
            .run(move |connection| {
                let current = shared.watermarks.borrow().get(timeframe);
                if current.is_some_and(|current| current >= through) {
                    return Ok(());
                }
                if timeframe == Timeframe::Day {
                    write::daily_watermark(connection, through)?;
                }
                shared.watermarks.send_modify(|watermarks| match timeframe {
                    Timeframe::Day => watermarks.day = Some(through),
                    Timeframe::Minute => watermarks.minute = Some(through),
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

    pub async fn traded_bars(
        &self,
        timeframe: Timeframe,
        symbols: Vec<Symbol>,
        from: Timestamp,
        to: Timestamp,
    ) -> Result<Vec<Bar>> {
        let listings = self.shared.listings();
        self.readers
            .run(move |connection| {
                read::traded_bars(
                    connection,
                    &listings,
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
        let (calendar, listings, actions) = (
            self.shared.calendar(),
            self.shared.listings(),
            self.shared.actions(),
        );
        self.readers
            .run(move |connection| {
                read::history(connection, &calendar, &listings, &actions, &request)
            })
            .await
    }

    pub async fn split_adjusted_closes(
        &self,
        symbols: Vec<Symbol>,
        as_of: Timestamp,
    ) -> Result<BTreeMap<Symbol, Decimal>> {
        let (calendar, listings, actions) = (
            self.shared.calendar(),
            self.shared.listings(),
            self.shared.actions(),
        );
        self.readers
            .run(move |connection| {
                read::split_adjusted_closes(
                    connection, &calendar, &listings, &actions, &symbols, as_of,
                )
            })
            .await
    }

    pub async fn chart(&self, request: ChartRequest) -> Result<ChartBars> {
        let (calendar, listings, actions) = (
            self.shared.calendar(),
            self.shared.listings(),
            self.shared.actions(),
        );
        self.readers
            .run(move |connection| {
                read::chart(connection, &calendar, &listings, &actions, &request)
            })
            .await
    }

    pub async fn fundamentals(
        &self,
        symbols: Vec<Symbol>,
        metrics: Vec<FundamentalMetric>,
        as_of: Timestamp,
    ) -> Result<Vec<FundamentalPoint>> {
        let (calendar, listings) = (self.shared.calendar(), self.shared.listings());
        self.readers
            .run(move |connection| {
                let resolved =
                    read::resolve(&listings, &symbols, read::listed_on(&calendar, as_of));
                let points =
                    read::fundamentals(connection, &read::keys(&resolved), &metrics, as_of)?;
                Ok(read::relabel(&resolved, points, |point| &mut point.symbol))
            })
            .await
    }

    pub async fn corporate_actions(
        &self,
        symbols: Vec<Symbol>,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<CorporateAction>> {
        Ok(self.shared.actions().between(&symbols, from, to))
    }

    pub async fn traded_actions(
        &self,
        symbols: Vec<Symbol>,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<CorporateAction>> {
        Ok(read::traded_actions(
            &self.shared.actions(),
            &self.shared.calendar(),
            &self.shared.listings(),
            &symbols,
            from,
            to,
        ))
    }

    pub async fn average_daily_volume(
        &self,
        symbols: Vec<Symbol>,
        before: NaiveDate,
        sessions: u32,
    ) -> Result<BTreeMap<Symbol, u64>> {
        let (calendar, listings, actions) = (
            self.shared.calendar(),
            self.shared.listings(),
            self.shared.actions(),
        );
        self.readers
            .run(move |connection| {
                read::average_daily_volume(
                    connection, &calendar, &listings, &actions, &symbols, before, sessions,
                )
            })
            .await
    }

    pub async fn universe(
        &self,
        spec: &UniverseSpec,
        date: NaiveDate,
        sessions: u32,
    ) -> Result<Vec<Symbol>> {
        let spec = spec.clone();
        let (calendar, listings) = (self.shared.calendar(), self.shared.listings());
        self.readers
            .run(move |connection| {
                let listed = match spec {
                    UniverseSpec::Symbols { symbols } => return Ok(symbols),
                    UniverseSpec::TopDollarVolume {
                        top_dollar_volume,
                        asset_class,
                    } => read::top_dollar_volume(
                        connection,
                        &calendar,
                        &listings,
                        top_dollar_volume,
                        asset_class,
                        date,
                        sessions,
                    )?,
                    UniverseSpec::Index { index } => read::index_universe(
                        connection, &calendar, &listings, index, date, sessions,
                    )?,
                };
                let keys: Vec<Symbol> = listed.iter().map(|(_, key)| key.clone()).collect();
                let delisted = read::delisted_before(connection, &keys, date)?;
                let mut symbols: Vec<Symbol> = listed
                    .into_iter()
                    .filter(|(_, key)| !delisted.contains(key))
                    .map(|(ticker, _)| ticker)
                    .collect();
                symbols.dedup();
                Ok(symbols)
            })
            .await
    }

    pub async fn index_history(&self, index: StockIndex) -> Result<Option<IndexHistory>> {
        self.readers
            .run(move |connection| read::index_history(connection, index))
            .await
    }

    pub async fn companies(&self) -> Result<Vec<Company>> {
        self.readers
            .run(|connection| read::companies(connection))
            .await
    }

    pub async fn instrument_profiles(
        &self,
        symbols: Vec<Symbol>,
        as_of: Timestamp,
    ) -> Result<Vec<InstrumentProfile>> {
        let (calendar, listings) = (self.shared.calendar(), self.shared.listings());
        self.readers
            .run(move |connection| {
                let resolved =
                    read::resolve(&listings, &symbols, read::listed_on(&calendar, as_of));
                let profiles = read::instrument_profiles(connection, &read::keys(&resolved))?;
                Ok(read::relabel(&resolved, profiles, |profile| {
                    &mut profile.symbol
                }))
            })
            .await
    }

    pub async fn earnings_releases(
        &self,
        symbols: Vec<Symbol>,
        as_of: Timestamp,
    ) -> Result<Vec<EarningsRelease>> {
        let (calendar, listings) = (self.shared.calendar(), self.shared.listings());
        self.readers
            .run(move |connection| {
                let resolved =
                    read::resolve(&listings, &symbols, read::listed_on(&calendar, as_of));
                let releases = read::earnings_releases(connection, &read::keys(&resolved), as_of)?;
                Ok(read::relabel(&resolved, releases, |release| {
                    &mut release.symbol
                }))
            })
            .await
    }

    pub async fn earnings_accessions(&self, symbol: Symbol) -> Result<BTreeSet<String>> {
        self.readers
            .run(move |connection| read::earnings_accessions(connection, &symbol))
            .await
    }

    pub async fn insider_accessions(&self, symbol: Symbol) -> Result<BTreeSet<String>> {
        self.readers
            .run(move |connection| read::insider_accessions(connection, &symbol))
            .await
    }

    pub async fn insider_trades(
        &self,
        symbols: Vec<Symbol>,
        since: NaiveDate,
        as_of: Timestamp,
    ) -> Result<Vec<InsiderTrade>> {
        let (calendar, listings) = (self.shared.calendar(), self.shared.listings());
        self.readers
            .run(move |connection| {
                let resolved =
                    read::resolve(&listings, &symbols, read::listed_on(&calendar, as_of));
                let keys = read::keys(&resolved);
                let trades = read::insider_trades(connection, &calendar, &keys, since, as_of)?;
                Ok(read::relabel(&resolved, trades, |trade| &mut trade.symbol))
            })
            .await
    }

    pub async fn news(
        &self,
        symbols: Vec<Symbol>,
        since: NaiveDate,
        as_of: Timestamp,
    ) -> Result<Vec<NewsArticle>> {
        self.readers
            .run(move |connection| read::news(connection, &symbols, since, as_of))
            .await
    }

    pub async fn latest_borrow(&self) -> Result<Option<Timestamp>> {
        self.readers
            .run(|connection| read::latest_borrow(connection))
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

    pub async fn fomc_meetings(&self) -> Result<Vec<FomcMeeting>> {
        self.readers
            .run(|connection| read::fomc_meetings(connection))
            .await
    }

    pub async fn fomc_announcements(
        &self,
        from: NaiveDate,
        to: NaiveDate,
        as_of: Timestamp,
    ) -> Result<Vec<FomcAnnouncement>> {
        let calendar = self.shared.calendar();
        self.readers
            .run(move |connection| read::fomc_announcements(connection, &calendar, from, to, as_of))
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
        let (calendar, listings) = (self.shared.calendar(), self.shared.listings());
        self.readers
            .run(move |connection| {
                read::coverage_gaps(
                    connection, &calendar, &listings, &symbols, timeframe, from, to,
                )
            })
            .await
    }

    pub async fn long_silences(&self, sessions: i64) -> Result<Vec<(Symbol, NaiveDate)>> {
        self.readers
            .run(move |connection| read::long_silences(connection, sessions))
            .await
    }

    pub async fn unpriced_members(
        &self,
        index: StockIndex,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<CoverageGap>> {
        let (calendar, listings) = (self.shared.calendar(), self.shared.listings());
        self.readers
            .run(move |connection| {
                read::unpriced_members(connection, &calendar, &listings, index, from, to)
            })
            .await
    }

    pub async fn member_warnings(
        &self,
        index: StockIndex,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<DataWarning>> {
        let shared = &self.shared;
        let (calendar, actions, listings) =
            (shared.calendar(), shared.actions(), shared.listings());
        self.readers
            .run(move |connection| {
                read::member_warnings(connection, &calendar, &actions, &listings, index, from, to)
            })
            .await
    }

    pub async fn unexplained_moves(
        &self,
        symbols: Vec<Symbol>,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<DataWarning>> {
        let (calendar, actions) = (self.shared.calendar(), self.shared.actions());
        self.readers
            .run(move |connection| {
                read::unexplained_moves(connection, &calendar, &actions, &symbols, from, to)
            })
            .await
    }

    pub async fn action_disagreements(
        &self,
        symbols: Option<Vec<Symbol>>,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<DataWarning>> {
        self.readers
            .run(move |connection| {
                read::action_disagreements(connection, symbols.as_deref(), from, to)
            })
            .await
    }

    pub async fn record_action_disagreements(
        &self,
        source: DataSource,
        disagreements: Vec<ActionDisagreement>,
    ) -> Result<WriteSummary> {
        self.writer
            .run(move |connection| write::action_checks(connection, source, disagreements))
            .await
    }

    pub async fn normalize_splits(&self, source: DataSource) -> Result<u64> {
        let shared = Arc::clone(&self.shared);
        self.writer
            .run(move |connection| {
                let normalized = write::normalized_splits(connection, source)?;
                if normalized > 0 {
                    shared.refresh_actions(connection)?;
                }
                Ok(normalized)
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

fn keep_daily_bars_ordered(connection: &mut Connection) -> Result<()> {
    let started = std::time::Instant::now();
    if let Some(rows) = write::order_daily_bars(connection)? {
        tracing::info!(
            rows,
            seconds = started.elapsed().as_secs_f64(),
            "put the daily bars back in date order"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_revision_table_is_backed_up() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate::migrate(&mut connection).unwrap();
        let mut statement = connection
            .prepare(
                "SELECT table_name FROM duckdb_tables() WHERE schema_name = 'main' \
                 AND table_name LIKE '%\\_revision' ESCAPE '\\' ORDER BY table_name",
            )
            .unwrap();
        let revisions: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<duckdb::Result<_>>()
            .unwrap();
        assert_eq!(revisions.len(), 13);
        let missing: Vec<&String> = revisions
            .iter()
            .filter(|table| !APPENDED.iter().any(|(backed_up, _)| backed_up == table))
            .collect();
        assert!(missing.is_empty(), "not backed up: {missing:?}");
    }
}
