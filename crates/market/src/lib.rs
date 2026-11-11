use std::collections::BTreeMap;
use std::path::Path;

use buizz_core::market::{
    Adjustment, Bar, Calendar, CorporateAction, Coverage, DataSource, FundamentalFact,
    FundamentalMetric, FundamentalPoint, Instrument, RiskFreeRate, Session, Symbol, Timeframe,
    Watermarks,
};
use buizz_core::ops::{CoverageGap, DataWarning};
use buizz_core::protocol::BarRow;
use buizz_core::time::Timestamp;
use chrono::NaiveDate;
use tokio::sync::watch;

pub const SCHEMA: &str = include_str!("../schema.sql");
pub const FILE_NAME: &str = "market.duckdb";

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

#[derive(Clone)]
pub struct MarketStore {}

impl MarketStore {
    pub async fn open(_path: &Path, _readers: usize) -> Result<Self> {
        unimplemented!()
    }

    pub async fn load_fixture(&self, _dir: &Path) -> Result<()> {
        unimplemented!()
    }

    pub async fn upsert_instruments(
        &self,
        _source: DataSource,
        _instruments: Vec<Instrument>,
    ) -> Result<WriteSummary> {
        unimplemented!()
    }

    pub async fn upsert_sessions(
        &self,
        _source: DataSource,
        _sessions: Vec<Session>,
    ) -> Result<WriteSummary> {
        unimplemented!()
    }

    pub async fn upsert_bars(
        &self,
        _source: DataSource,
        _timeframe: Timeframe,
        _bars: Vec<Bar>,
    ) -> Result<WriteSummary> {
        unimplemented!()
    }

    pub async fn upsert_corporate_actions(
        &self,
        _source: DataSource,
        _actions: Vec<CorporateAction>,
    ) -> Result<WriteSummary> {
        unimplemented!()
    }

    pub async fn append_fundamental_facts(
        &self,
        _source: DataSource,
        _facts: Vec<FundamentalFact>,
    ) -> Result<WriteSummary> {
        unimplemented!()
    }

    pub async fn upsert_risk_free_rates(
        &self,
        _source: DataSource,
        _rates: Vec<RiskFreeRate>,
    ) -> Result<WriteSummary> {
        unimplemented!()
    }

    pub async fn advance_watermark(&self, _timeframe: Timeframe, _through: Timestamp) -> Result<()> {
        unimplemented!()
    }

    pub fn watermarks(&self) -> watch::Receiver<Watermarks> {
        unimplemented!()
    }

    pub async fn calendar(&self, _from: NaiveDate, _to: NaiveDate) -> Result<Calendar> {
        unimplemented!()
    }

    pub async fn instruments(&self) -> Result<Vec<Instrument>> {
        unimplemented!()
    }

    pub async fn bars_between(
        &self,
        _timeframe: Timeframe,
        _symbols: Vec<Symbol>,
        _from: Timestamp,
        _to: Timestamp,
    ) -> Result<Vec<Bar>> {
        unimplemented!()
    }

    pub async fn history(&self, _request: HistoryRequest) -> Result<Vec<BarRow>> {
        unimplemented!()
    }

    pub async fn fundamentals(
        &self,
        _symbols: Vec<Symbol>,
        _metrics: Vec<FundamentalMetric>,
        _as_of: Timestamp,
    ) -> Result<Vec<FundamentalPoint>> {
        unimplemented!()
    }

    pub async fn corporate_actions(
        &self,
        _symbols: Vec<Symbol>,
        _from: NaiveDate,
        _to: NaiveDate,
    ) -> Result<Vec<CorporateAction>> {
        unimplemented!()
    }

    pub async fn average_daily_volume(
        &self,
        _symbols: Vec<Symbol>,
        _before: NaiveDate,
        _sessions: u32,
    ) -> Result<BTreeMap<Symbol, u64>> {
        unimplemented!()
    }

    pub async fn top_dollar_volume(
        &self,
        _count: u32,
        _before: NaiveDate,
        _sessions: u32,
    ) -> Result<Vec<Symbol>> {
        unimplemented!()
    }

    pub async fn risk_free_rates(&self, _from: NaiveDate, _to: NaiveDate) -> Result<Vec<RiskFreeRate>> {
        unimplemented!()
    }

    pub async fn coverage(&self) -> Result<Vec<Coverage>> {
        unimplemented!()
    }

    pub async fn coverage_gaps(
        &self,
        _symbols: Vec<Symbol>,
        _timeframe: Timeframe,
        _from: NaiveDate,
        _to: NaiveDate,
    ) -> Result<Vec<CoverageGap>> {
        unimplemented!()
    }

    pub async fn unexplained_moves(
        &self,
        _symbols: Vec<Symbol>,
        _from: NaiveDate,
        _to: NaiveDate,
    ) -> Result<Vec<DataWarning>> {
        unimplemented!()
    }
}
