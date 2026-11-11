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
