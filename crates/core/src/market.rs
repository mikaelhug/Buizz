use std::fmt;
use std::str::FromStr;
use std::time::Duration;

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

    pub fn ended(ticker: &Symbol, last_session: NaiveDate) -> Self {
        Self(format!("{ticker}@{last_session}"))
    }

    pub fn ended_listing(&self) -> Option<(Symbol, NaiveDate)> {
        let (ticker, last_session) = self.0.split_once('@')?;
        Some((Self(ticker.to_owned()), last_session.parse().ok()?))
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

pub const SIP_DELAY: Duration = Duration::from_secs(15 * 60);
pub const ALPACA_HISTORY_START: NaiveDate =
    NaiveDate::from_ymd_opt(2016, 1, 1).expect("valid date");
pub const HISTORY_START: NaiveDate = NaiveDate::from_ymd_opt(2000, 1, 1).expect("valid date");
pub const HISTORY_FLOORS: [NaiveDate; 2] = [HISTORY_START, ALPACA_HISTORY_START];

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

    pub fn is_closed_at(&self, at: Timestamp) -> bool {
        at >= self.close
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
    StockDividend {
        rate: Decimal,
    },
    SpinOff {
        new_symbol: Symbol,
        old_rate: Decimal,
        new_rate: Decimal,
    },
    CashMerger {
        rate: Decimal,
    },
    StockMerger {
        acquirer: Symbol,
        old_rate: Decimal,
        new_rate: Decimal,
        cash_rate: Decimal,
    },
    WorthlessRemoval,
    NameChange {
        new_symbol: Symbol,
    },
}

impl CorporateActionKind {
    pub fn share_ratio(&self) -> Option<(Decimal, Decimal)> {
        match self {
            Self::Split { old_rate, new_rate } => Some((*old_rate, *new_rate)),
            Self::StockDividend { rate } => Some((Decimal::ONE, Decimal::ONE + *rate)),
            _ => None,
        }
    }

    pub fn delivered(&self) -> Option<&Symbol> {
        match self {
            Self::SpinOff { new_symbol, .. } => Some(new_symbol),
            Self::StockMerger { acquirer, .. } => Some(acquirer),
            _ => None,
        }
    }

    pub fn ends_listing(&self) -> bool {
        matches!(
            self,
            Self::CashMerger { .. }
                | Self::StockMerger { .. }
                | Self::WorthlessRemoval
                | Self::NameChange { .. }
        )
    }

    pub fn has_valid_rates(&self) -> bool {
        let positive = |rates: &[Decimal]| rates.iter().all(|rate| *rate > Decimal::ZERO);
        match self {
            Self::Split { old_rate, new_rate }
            | Self::SpinOff {
                old_rate, new_rate, ..
            }
            | Self::StockMerger {
                old_rate, new_rate, ..
            } => positive(&[*old_rate, *new_rate]),
            Self::StockDividend { rate } => positive(&[*rate]),
            Self::CashDividend { rate } | Self::CashMerger { rate } => *rate >= Decimal::ZERO,
            Self::WorthlessRemoval | Self::NameChange { .. } => true,
        }
    }
}

const MAX_SPLIT_DENOMINATOR: i64 = 100;

pub fn split_rates(factor: Decimal) -> (Decimal, Decimal) {
    let tolerance = Decimal::new(1, 4);
    (1..=MAX_SPLIT_DENOMINATOR)
        .map(Decimal::from)
        .find_map(|old_rate| {
            let new_rate = (factor * old_rate).round();
            let close =
                !new_rate.is_zero() && ((new_rate / old_rate - factor) / factor).abs() <= tolerance;
            close.then_some((old_rate, new_rate))
        })
        .unwrap_or((Decimal::ONE, factor))
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
    pub fn for_shares(action: &CorporateAction, adjustment: Adjustment) -> Option<Self> {
        if adjustment == Adjustment::Raw || !action.kind.has_valid_rates() {
            return None;
        }
        let (old_rate, new_rate) = action.kind.share_ratio()?;
        Some(Self {
            ex_date: action.ex_date,
            price: old_rate / new_rate,
            volume: new_rate / old_rate,
        })
    }

    pub fn for_distribution(
        ex_date: NaiveDate,
        value: Decimal,
        previous_close: Decimal,
    ) -> Option<Self> {
        if previous_close <= Decimal::ZERO || value < Decimal::ZERO || value >= previous_close {
            return None;
        }
        Some(Self {
            ex_date,
            price: Decimal::ONE - value / previous_close,
            volume: Decimal::ONE,
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
    ShortTermDebt,
    ShortTermBorrowings,
    CurrentLongTermDebt,
    LongTermDebtNoncurrent,
    PreferredStock,
    MinorityInterest,
    RetainedEarnings,
    IncomeTaxesPayable,
    ShortTermInvestments,
    ConvertibleDebt,
    SecuredDebt,
    Land,
    Buildings,
    PpeGross,
    Advertising,
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
    pub unit: String,
    pub form: String,
    pub filed: NaiveDate,
    pub available_at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Company {
    pub symbol: Symbol,
    pub cik: u32,
    pub sic: Option<u16>,
    pub sic_description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct InstrumentProfile {
    pub symbol: Symbol,
    pub name: String,
    pub exchange: String,
    pub asset_class: AssetClass,
    pub sic: Option<u16>,
    pub sic_description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecFiling {
    pub cik: u32,
    pub accession: String,
    pub form: String,
    pub filed: NaiveDate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EarningsRelease {
    pub symbol: Symbol,
    pub cik: u32,
    pub accession: String,
    pub form: String,
    pub filed: NaiveDate,
    pub report_date: Option<NaiveDate>,
    pub accepted_at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsiderOwner {
    pub cik: u32,
    pub name: String,
    pub director: bool,
    pub officer: bool,
    pub ten_percent_owner: bool,
    pub other: bool,
    pub officer_title: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsiderTransaction {
    pub security: String,
    pub date: NaiveDate,
    pub code: String,
    pub acquired: bool,
    pub shares: Option<Decimal>,
    pub price: Option<Decimal>,
    pub shares_after: Option<Decimal>,
    pub direct: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InsiderFiling {
    pub symbol: Symbol,
    pub issuer_cik: u32,
    pub accession: String,
    pub form: String,
    pub filed: NaiveDate,
    pub accepted_at: Timestamp,
    pub owners: Vec<InsiderOwner>,
    pub transactions: Vec<InsiderTransaction>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsiderTrade {
    pub symbol: Symbol,
    pub accession: String,
    pub form: String,
    pub filed: NaiveDate,
    pub accepted_at: Timestamp,
    pub available_at: Timestamp,
    pub owners: Vec<InsiderOwner>,
    pub transaction: InsiderTransaction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsArticle {
    pub id: u64,
    pub symbols: Vec<Symbol>,
    pub headline: String,
    pub summary: String,
    pub source: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub received_at: Timestamp,
    pub backfilled: bool,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
pub enum StockIndex {
    #[serde(rename = "sp500")]
    Sp500,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct IndexMember {
    pub index: StockIndex,
    pub symbol: Symbol,
    pub start: Option<NaiveDate>,
    pub end: Option<NaiveDate>,
}

impl IndexMember {
    pub fn member_on(&self, date: NaiveDate) -> bool {
        self.start.is_none_or(|start| start <= date) && self.end.is_none_or(|end| date < end)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct IndexHistory {
    pub index: StockIndex,
    pub members: Vec<IndexMember>,
    pub changes: u32,
    pub reliable_from: Option<NaiveDate>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct BorrowAvailability {
    pub symbol: Symbol,
    pub at: Timestamp,
    pub con_id: u64,
    pub rebate_rate: Option<Decimal>,
    pub fee_rate: Option<Decimal>,
    pub available: u64,
    pub available_or_more: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RiskFreeRate {
    pub date: NaiveDate,
    pub percent: Decimal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum MacroSeries {
    #[serde(rename = "treasury_3m")]
    Treasury3m,
    #[serde(rename = "tb3ms")]
    Tb3ms,
    #[serde(rename = "gs10")]
    Gs10,
    #[serde(rename = "aaa")]
    Aaa,
    #[serde(rename = "baa")]
    Baa,
    #[serde(rename = "cpiaucns")]
    Cpiaucns,
    #[serde(rename = "unrate")]
    Unrate,
    #[serde(rename = "sahmrealtime")]
    Sahmrealtime,
    #[serde(rename = "dgs10")]
    Dgs10,
    #[serde(rename = "t10y3m")]
    T10y3m,
    #[serde(rename = "baa10y")]
    Baa10y,
    #[serde(rename = "icsa")]
    Icsa,
    #[serde(rename = "ccsa")]
    Ccsa,
    #[serde(rename = "indpro")]
    Indpro,
    #[serde(rename = "permit")]
    Permit,
    #[serde(rename = "rsafs")]
    Rsafs,
    #[serde(rename = "vix")]
    Vix,
    #[serde(rename = "vix3m")]
    Vix3m,
    #[serde(rename = "sp500_forward_pe")]
    Sp500ForwardPe,
    #[serde(rename = "sp500_forward_pe_5y_avg")]
    Sp500ForwardPe5yAvg,
    #[serde(rename = "sp500_forward_pe_10y_avg")]
    Sp500ForwardPe10yAvg,
    #[serde(rename = "sp500_forward_eps")]
    Sp500ForwardEps,
    #[serde(rename = "sp500_trailing_pe")]
    Sp500TrailingPe,
    #[serde(rename = "sp500_energy_forward_pe")]
    Sp500EnergyForwardPe,
    #[serde(rename = "sp500_materials_forward_pe")]
    Sp500MaterialsForwardPe,
    #[serde(rename = "sp500_industrials_forward_pe")]
    Sp500IndustrialsForwardPe,
    #[serde(rename = "sp500_consumer_discretionary_forward_pe")]
    Sp500ConsumerDiscretionaryForwardPe,
    #[serde(rename = "sp500_consumer_staples_forward_pe")]
    Sp500ConsumerStaplesForwardPe,
    #[serde(rename = "sp500_health_care_forward_pe")]
    Sp500HealthCareForwardPe,
    #[serde(rename = "sp500_financials_forward_pe")]
    Sp500FinancialsForwardPe,
    #[serde(rename = "sp500_information_technology_forward_pe")]
    Sp500InformationTechnologyForwardPe,
    #[serde(rename = "sp500_communication_services_forward_pe")]
    Sp500CommunicationServicesForwardPe,
    #[serde(rename = "sp500_telecom_services_forward_pe")]
    Sp500TelecomServicesForwardPe,
    #[serde(rename = "sp500_utilities_forward_pe")]
    Sp500UtilitiesForwardPe,
    #[serde(rename = "sp500_real_estate_forward_pe")]
    Sp500RealEstateForwardPe,
}

impl MacroSeries {
    pub const FRED: [Self; 15] = [
        Self::Tb3ms,
        Self::Gs10,
        Self::Aaa,
        Self::Baa,
        Self::Cpiaucns,
        Self::Unrate,
        Self::Sahmrealtime,
        Self::Dgs10,
        Self::T10y3m,
        Self::Baa10y,
        Self::Icsa,
        Self::Ccsa,
        Self::Indpro,
        Self::Permit,
        Self::Rsafs,
    ];
    pub const CBOE: [Self; 2] = [Self::Vix, Self::Vix3m];

    pub fn fred_id(self) -> Option<&'static str> {
        match self {
            Self::Treasury3m | Self::Vix | Self::Vix3m => None,
            Self::Tb3ms => Some("TB3MS"),
            Self::Gs10 => Some("GS10"),
            Self::Aaa => Some("AAA"),
            Self::Baa => Some("BAA"),
            Self::Cpiaucns => Some("CPIAUCNS"),
            Self::Unrate => Some("UNRATE"),
            Self::Sahmrealtime => Some("SAHMREALTIME"),
            Self::Dgs10 => Some("DGS10"),
            Self::T10y3m => Some("T10Y3M"),
            Self::Baa10y => Some("BAA10Y"),
            Self::Icsa => Some("ICSA"),
            Self::Ccsa => Some("CCSA"),
            Self::Indpro => Some("INDPRO"),
            Self::Permit => Some("PERMIT"),
            Self::Rsafs => Some("RSAFS"),
            Self::Sp500ForwardPe
            | Self::Sp500ForwardPe5yAvg
            | Self::Sp500ForwardPe10yAvg
            | Self::Sp500ForwardEps
            | Self::Sp500TrailingPe
            | Self::Sp500EnergyForwardPe
            | Self::Sp500MaterialsForwardPe
            | Self::Sp500IndustrialsForwardPe
            | Self::Sp500ConsumerDiscretionaryForwardPe
            | Self::Sp500ConsumerStaplesForwardPe
            | Self::Sp500HealthCareForwardPe
            | Self::Sp500FinancialsForwardPe
            | Self::Sp500InformationTechnologyForwardPe
            | Self::Sp500CommunicationServicesForwardPe
            | Self::Sp500TelecomServicesForwardPe
            | Self::Sp500UtilitiesForwardPe
            | Self::Sp500RealEstateForwardPe => None,
        }
    }

    pub fn estimate(self) -> Option<(EstimateMetric, EstimateScope)> {
        let sector = |scope| Some((EstimateMetric::ForwardPe, scope));
        match self {
            Self::Treasury3m
            | Self::Tb3ms
            | Self::Gs10
            | Self::Aaa
            | Self::Baa
            | Self::Cpiaucns
            | Self::Unrate
            | Self::Sahmrealtime
            | Self::Dgs10
            | Self::T10y3m
            | Self::Baa10y
            | Self::Icsa
            | Self::Ccsa
            | Self::Indpro
            | Self::Permit
            | Self::Rsafs
            | Self::Vix
            | Self::Vix3m => None,
            Self::Sp500ForwardPe => sector(EstimateScope::Sp500),
            Self::Sp500ForwardPe5yAvg => {
                Some((EstimateMetric::ForwardPe5yAvg, EstimateScope::Sp500))
            }
            Self::Sp500ForwardPe10yAvg => {
                Some((EstimateMetric::ForwardPe10yAvg, EstimateScope::Sp500))
            }
            Self::Sp500ForwardEps => Some((EstimateMetric::ForwardEps, EstimateScope::Sp500)),
            Self::Sp500TrailingPe => Some((EstimateMetric::TrailingPe, EstimateScope::Sp500)),
            Self::Sp500EnergyForwardPe => sector(EstimateScope::Energy),
            Self::Sp500MaterialsForwardPe => sector(EstimateScope::Materials),
            Self::Sp500IndustrialsForwardPe => sector(EstimateScope::Industrials),
            Self::Sp500ConsumerDiscretionaryForwardPe => {
                sector(EstimateScope::ConsumerDiscretionary)
            }
            Self::Sp500ConsumerStaplesForwardPe => sector(EstimateScope::ConsumerStaples),
            Self::Sp500HealthCareForwardPe => sector(EstimateScope::HealthCare),
            Self::Sp500FinancialsForwardPe => sector(EstimateScope::Financials),
            Self::Sp500InformationTechnologyForwardPe => {
                sector(EstimateScope::InformationTechnology)
            }
            Self::Sp500CommunicationServicesForwardPe => {
                sector(EstimateScope::CommunicationServices)
            }
            Self::Sp500TelecomServicesForwardPe => sector(EstimateScope::TelecomServices),
            Self::Sp500UtilitiesForwardPe => sector(EstimateScope::Utilities),
            Self::Sp500RealEstateForwardPe => sector(EstimateScope::RealEstate),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstimateMetric {
    ForwardPe,
    ForwardPe5yAvg,
    ForwardPe10yAvg,
    ForwardEps,
    IndexPrice,
    TrailingPe,
    TrailingPe5yAvg,
    TrailingPe10yAvg,
    EpsGrowth,
    RevenueGrowth,
    EpsBeatPct,
    RevenueBeatPct,
    NegativeGuidance,
    PositiveGuidance,
    TargetPrice,
    TargetUpsidePct,
    BuyPct,
    HoldPct,
    SellPct,
    Ratings,
}

impl EstimateMetric {
    pub fn unit(self) -> &'static str {
        match self {
            Self::ForwardPe
            | Self::ForwardPe5yAvg
            | Self::ForwardPe10yAvg
            | Self::TrailingPe
            | Self::TrailingPe5yAvg
            | Self::TrailingPe10yAvg => "ratio",
            Self::ForwardEps => "usd",
            Self::IndexPrice | Self::TargetPrice => "points",
            Self::EpsGrowth
            | Self::RevenueGrowth
            | Self::EpsBeatPct
            | Self::RevenueBeatPct
            | Self::TargetUpsidePct
            | Self::BuyPct
            | Self::HoldPct
            | Self::SellPct => "percent",
            Self::NegativeGuidance | Self::PositiveGuidance | Self::Ratings => "count",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstimateScope {
    Sp500,
    Energy,
    Materials,
    Industrials,
    ConsumerDiscretionary,
    ConsumerStaples,
    HealthCare,
    Financials,
    InformationTechnology,
    CommunicationServices,
    TelecomServices,
    Utilities,
    RealEstate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EstimatePeriod {
    Year(i32),
    Quarter(i32, u32),
}

impl fmt::Display for EstimatePeriod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Year(year) => write!(f, "{year}"),
            Self::Quarter(year, quarter) => write!(f, "{year}Q{quarter}"),
        }
    }
}

impl FromStr for EstimatePeriod {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || format!("{text:?} is not a year or a quarter like 2026Q3");
        match text.split_once('Q') {
            Some((year, quarter)) => {
                let quarter: u32 = quarter.parse().map_err(|_| invalid())?;
                (1..=4)
                    .contains(&quarter)
                    .then_some(Self::Quarter(year.parse().map_err(|_| invalid())?, quarter))
                    .ok_or_else(invalid)
            }
            None => text.parse().map(Self::Year).map_err(|_| invalid()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EstimateFact {
    pub metric: EstimateMetric,
    pub scope: EstimateScope,
    pub period: Option<EstimatePeriod>,
    pub as_of: Option<NaiveDate>,
    pub value: Decimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EarningsInsight {
    pub report: String,
    pub issue_date: NaiveDate,
    pub published: NaiveDate,
    pub url: String,
    pub retrieved_from: String,
    pub digest: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MacroObservation {
    pub series: MacroSeries,
    pub date: NaiveDate,
    pub published: NaiveDate,
    pub value: Option<Decimal>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroPoint {
    pub series: MacroSeries,
    pub date: NaiveDate,
    pub value: Decimal,
    pub published: NaiveDate,
    pub available_at: Timestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FomcMeeting {
    pub date: NaiveDate,
    pub scheduled: bool,
    pub statement_at: Option<Timestamp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FomcAnnouncement {
    pub date: NaiveDate,
    pub scheduled: bool,
    pub statement_at: Option<Timestamp>,
    pub available_at: Timestamp,
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
    Fred,
    Fixture,
    Wikipedia,
    InteractiveBrokers,
    Nasdaq,
    Cboe,
    FederalReserve,
    Factset,
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
    Companies,
    EarningsReleases,
    SecFilings,
    IndexMembership,
    Borrow,
    Macro,
    InsiderTrades,
    News,
    Fomc,
    Estimates,
}

impl Dataset {
    pub const ALL: [Self; 17] = [
        Self::Bars1d,
        Self::Bars1m,
        Self::CorporateActions,
        Self::Instruments,
        Self::Calendar,
        Self::IndexMembership,
        Self::Companies,
        Self::Fundamentals,
        Self::SecFilings,
        Self::EarningsReleases,
        Self::InsiderTrades,
        Self::Estimates,
        Self::Rates,
        Self::Macro,
        Self::Fomc,
        Self::News,
        Self::Borrow,
    ];

    pub fn domain(self) -> DataDomain {
        match self {
            Self::Bars1d | Self::Bars1m | Self::CorporateActions => DataDomain::Prices,
            Self::Instruments | Self::Calendar | Self::IndexMembership | Self::Companies => {
                DataDomain::Reference
            }
            Self::Fundamentals
            | Self::SecFilings
            | Self::EarningsReleases
            | Self::InsiderTrades
            | Self::Estimates => DataDomain::Filings,
            Self::Rates | Self::Macro | Self::Fomc => DataDomain::Macro,
            Self::News | Self::Borrow => DataDomain::Alternative,
        }
    }
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DataDomain {
    Prices,
    Reference,
    Filings,
    Macro,
    Alternative,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Market {
    Us,
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
