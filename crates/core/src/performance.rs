use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::market::Symbol;
use crate::order::OrderId;
use crate::run::RunId;
use crate::time::Timestamp;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Metrics {
    pub total_return: Option<f64>,
    pub cagr: Option<f64>,
    pub volatility: Option<f64>,
    pub sharpe: Option<f64>,
    pub sortino: Option<f64>,
    pub max_drawdown: Option<f64>,
    pub calmar: Option<f64>,
    pub win_rate: Option<f64>,
    pub profit_factor: Option<f64>,
    pub exposure: Option<f64>,
    pub turnover: Option<f64>,
    pub alpha: Option<f64>,
    pub beta: Option<f64>,
    pub residual_volatility: Option<f64>,
    pub appraisal_ratio: Option<f64>,
    pub alpha_t: Option<f64>,
    pub overlay_sharpe: Option<f64>,
    pub hedge_alpha: Option<f64>,
    pub hedge_beta: Option<f64>,
    pub hedge_residual_volatility: Option<f64>,
    pub hedge_appraisal_ratio: Option<f64>,
    pub hedge_alpha_t: Option<f64>,
    pub trades: u32,
    pub sessions: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RoundTrip {
    pub symbol: Symbol,
    pub qty: i64,
    pub opened_at: Timestamp,
    pub closed_at: Timestamp,
    pub opened_by: Option<OrderId>,
    pub closed_by: Option<OrderId>,
    #[serde(with = "crate::money::cash")]
    pub pnl: Decimal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LiveVerdict {
    Incubating,
    Tracking,
    NoBacktest,
    Falsified,
    Proven,
}

impl LiveVerdict {
    pub const TRACKING_SESSIONS: u32 = 20;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvelopeBreach {
    Excess,
    Drawdown,
    Volatility,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Envelope {
    pub source: RunId,
    pub source_sessions: u32,
    pub windows: u32,
    pub excess_floor: Option<f64>,
    pub drawdown_ceiling: Option<f64>,
    pub volatility: Option<f64>,
    pub breaches: Vec<EnvelopeBreach>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SharpeMoments {
    pub sharpe: f64,
    pub skew: f64,
    pub kurtosis: f64,
    pub observations: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LivePoint {
    pub at: Timestamp,
    #[serde(with = "crate::money::cash")]
    pub equity: Decimal,
    #[serde(with = "crate::money::cash")]
    pub benchmark: Decimal,
    #[serde(default, with = "crate::money::cash_option")]
    pub floor: Option<Decimal>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LiveStats {
    pub through: NaiveDate,
    pub sessions: u32,
    pub paused_sessions: u32,
    pub cagr: f64,
    pub benchmark_cagr: f64,
    pub excess_cagr: f64,
    pub excess_cagr_error: Option<f64>,
    pub total_return: f64,
    pub benchmark_return: f64,
    pub excess_return: f64,
    pub excess_error: Option<f64>,
    pub max_drawdown: f64,
    pub volatility: Option<f64>,
    pub beta: Option<f64>,
    pub excess: Option<SharpeMoments>,
    pub probabilistic_sharpe: Option<f64>,
    pub verdict: LiveVerdict,
    pub envelope: Option<Envelope>,
    pub curve: Vec<LivePoint>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Cadence {
    Daily,
    Quarterly,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DrawdownResponse {
    pub label: String,
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub net_buying: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Fingerprint {
    pub cadence: Cadence,
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub observations: u32,
    pub positions_median: Option<f64>,
    pub positions_min: Option<u32>,
    pub positions_max: Option<u32>,
    pub top_five_share: Option<f64>,
    pub turnover: Option<f64>,
    pub holding_months_median: Option<f64>,
    pub holding_months_mean: Option<f64>,
    pub holdings: u32,
    pub stock_share: Option<f64>,
    pub idle_months: Option<f64>,
    pub idle_quarters: Option<f64>,
    pub buys_into_weakness: Option<f64>,
    pub sells_on_price: Option<f64>,
    pub drawdowns: Vec<DrawdownResponse>,
    pub characteristics: Characteristics,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Characteristics {
    pub beta: Option<f64>,
    pub book_to_market: Option<f64>,
    pub roe: Option<f64>,
    pub market_cap: Option<f64>,
    pub debt_to_equity: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ReferenceFingerprint {
    pub name: String,
    pub source: String,
    pub fingerprint: Fingerprint,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Behaviour {
    pub run: Option<Fingerprint>,
    pub references: Vec<ReferenceFingerprint>,
}
