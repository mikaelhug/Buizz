use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::market::Symbol;
use crate::order::OrderId;
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
}

impl LiveVerdict {
    pub const TRACKING_SESSIONS: u32 = 20;

    pub fn after(sessions: u32) -> Self {
        if sessions < Self::TRACKING_SESSIONS {
            Self::Incubating
        } else {
            Self::Tracking
        }
    }
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LiveStats {
    pub through: NaiveDate,
    pub sessions: u32,
    pub paused_sessions: u32,
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
    pub curve: Vec<LivePoint>,
}
