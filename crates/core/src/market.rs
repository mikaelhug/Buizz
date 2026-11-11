use std::fmt;
use std::str::FromStr;

use chrono::{NaiveDate, TimeDelta};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::time::Timestamp;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
#[schema(value_type = String, example = "AAPL")]
pub struct Symbol(String);

impl Symbol {
    pub fn new(text: &str) -> Self {
        Self(text.trim().to_ascii_uppercase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for Symbol {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AssetClass {
    Stock,
    Etf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Instrument {
    pub symbol: Symbol,
    pub name: String,
    pub exchange: String,
    pub asset_class: AssetClass,
    pub active: bool,
    pub shortable: bool,
    pub first_session: Option<NaiveDate>,
    pub last_session: Option<NaiveDate>,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
pub enum Timeframe {
    #[serde(rename = "1d")]
    Day,
    #[serde(rename = "1m")]
    Minute,
}

impl Timeframe {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "1d",
            Self::Minute => "1m",
        }
    }
}

impl fmt::Display for Timeframe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown timeframe {0:?}, expected 1d or 1m")]
pub struct ParseTimeframeError(String);

impl FromStr for Timeframe {
    type Err = ParseTimeframeError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "1d" => Ok(Self::Day),
            "1m" => Ok(Self::Minute),
            other => Err(ParseTimeframeError(other.to_owned())),
        }
    }
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Feed {
    Sip,
    Iex,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Bar {
    pub symbol: Symbol,
    pub start: Timestamp,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: u64,
    pub feed: Feed,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
pub struct Session {
    pub date: NaiveDate,
    pub open: Timestamp,
    pub close: Timestamp,
}

impl Session {
    pub fn minutes(&self) -> i64 {
        (self.close - self.open).num_minutes()
    }

    pub fn last_bar_start(&self, timeframe: Timeframe) -> Timestamp {
        match timeframe {
            Timeframe::Day => self.open,
            Timeframe::Minute => self.close - TimeDelta::minutes(1),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Calendar {
    sessions: Vec<Session>,
}

impl Calendar {
    pub fn new(mut sessions: Vec<Session>) -> Self {
        sessions.sort_by_key(|session| session.date);
        sessions.dedup_by_key(|session| session.date);
        Self { sessions }
    }

    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }

    pub fn session(&self, date: NaiveDate) -> Option<&Session> {
        self.sessions
            .binary_search_by_key(&date, |session| session.date)
            .ok()
            .map(|index| &self.sessions[index])
    }

    pub fn first_on_or_after(&self, date: NaiveDate) -> Option<&Session> {
        let index = self.sessions.partition_point(|session| session.date < date);
        self.sessions.get(index)
    }

    pub fn next_after(&self, date: NaiveDate) -> Option<&Session> {
        let index = self
            .sessions
            .partition_point(|session| session.date <= date);
        self.sessions.get(index)
    }

    pub fn previous_before(&self, date: NaiveDate) -> Option<&Session> {
        let index = self.sessions.partition_point(|session| session.date < date);
        index
            .checked_sub(1)
            .map(|previous| &self.sessions[previous])
    }

    pub fn containing(&self, at: Timestamp) -> Option<&Session> {
        let index = self.sessions.partition_point(|session| session.close < at);
        self.sessions
            .get(index)
            .filter(|session| session.open <= at)
    }

    pub fn between(&self, from: NaiveDate, to: NaiveDate) -> &[Session] {
        let start = self.sessions.partition_point(|session| session.date < from);
        let end = self.sessions.partition_point(|session| session.date <= to);
        &self.sessions[start..end.max(start)]
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CorporateAction {
    pub symbol: Symbol,
    pub ex_date: NaiveDate,
    pub kind: CorporateActionKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CorporateActionKind {
    Split {
        old_rate: Decimal,
        new_rate: Decimal,
    },
    CashDividend {
        rate: Decimal,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Adjustment {
    Raw,
    Split,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdjustmentFactor {
    pub ex_date: NaiveDate,
    pub price: Decimal,
    pub volume: Decimal,
}

impl AdjustmentFactor {
    pub fn for_action(
        action: &CorporateAction,
        adjustment: Adjustment,
        previous_close: Option<Decimal>,
    ) -> Option<Self> {
        let (price, volume) = match (&action.kind, adjustment) {
            (_, Adjustment::Raw) => return None,
            (CorporateActionKind::Split { old_rate, new_rate }, _) => {
                (*old_rate / *new_rate, *new_rate / *old_rate)
            }
            (CorporateActionKind::CashDividend { .. }, Adjustment::Split) => return None,
            (CorporateActionKind::CashDividend { rate }, Adjustment::All) => {
                let close = previous_close.filter(|close| !close.is_zero())?;
                (Decimal::ONE - *rate / close, Decimal::ONE)
            }
        };
        Some(Self {
            ex_date: action.ex_date,
            price,
            volume,
        })
    }

    pub fn cumulative(bar_date: NaiveDate, factors: &[Self]) -> (Decimal, Decimal) {
        factors
            .iter()
            .filter(|factor| factor.ex_date > bar_date)
            .fold((Decimal::ONE, Decimal::ONE), |(price, volume), factor| {
                (price * factor.price, volume * factor.volume)
            })
    }
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum FundamentalMetric {
    Revenue,
    CostOfRevenue,
    GrossProfit,
    ResearchDevelopment,
    SgaExpense,
    OperatingExpenses,
    OperatingIncome,
    InterestExpense,
    PretaxIncome,
    IncomeTax,
    NetIncome,
    EpsDiluted,
    TotalAssets,
    CurrentAssets,
    CashAndEquivalents,
    Inventory,
    Receivables,
    PpeNet,
    Goodwill,
    CurrentLiabilities,
    LongTermDebt,
    TotalLiabilities,
    Equity,
    OperatingCashFlow,
    DepreciationAmortization,
    StockBasedComp,
    Capex,
    InvestingCashFlow,
    FinancingCashFlow,
    DividendsPaid,
    ShareRepurchases,
    Acquisitions,
    SharesOutstanding,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
pub enum PeriodType {
    #[serde(rename = "FY")]
    Annual,
    #[serde(rename = "Q")]
    Quarter,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FundamentalFact {
    pub symbol: Symbol,
    pub cik: u32,
    pub taxonomy: String,
    pub concept: String,
    pub unit: String,
    pub metric: Option<FundamentalMetric>,
    pub period_type: Option<PeriodType>,
    pub period_start: Option<NaiveDate>,
    pub period_end: NaiveDate,
    pub value: Decimal,
    pub fiscal_year: Option<i32>,
    pub fiscal_period: Option<String>,
    pub form: String,
    pub accession: String,
    pub filed: NaiveDate,
    pub frame: Option<String>,
    pub available_at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FundamentalPoint {
    pub symbol: Symbol,
    pub metric: FundamentalMetric,
    pub period_type: PeriodType,
    pub period_start: Option<NaiveDate>,
    pub period_end: NaiveDate,
    pub value: Decimal,
    pub form: String,
    pub filed: NaiveDate,
    pub available_at: Timestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RiskFreeRate {
    pub date: NaiveDate,
    pub percent: Decimal,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DataSource {
    Alpaca,
    AlpacaStream,
    Tiingo,
    Edgar,
    Treasury,
    Fixture,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Dataset {
    Instruments,
    Calendar,
    Bars1d,
    Bars1m,
    CorporateActions,
    Fundamentals,
    Rates,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Coverage {
    pub dataset: Dataset,
    pub symbol: Option<Symbol>,
    pub source: DataSource,
    pub first: NaiveDate,
    pub last: NaiveDate,
    pub updated_at: Timestamp,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Watermarks {
    pub day: Option<Timestamp>,
    pub minute: Option<Timestamp>,
}

impl Watermarks {
    pub fn get(&self, timeframe: Timeframe) -> Option<Timestamp> {
        match timeframe {
            Timeframe::Day => self.day,
            Timeframe::Minute => self.minute,
        }
    }
}
