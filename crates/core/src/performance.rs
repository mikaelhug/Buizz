use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Metrics {
    pub total_return: f64,
    pub cagr: f64,
    pub volatility: f64,
    pub sharpe: Option<f64>,
    pub sortino: Option<f64>,
    pub max_drawdown: f64,
    pub calmar: Option<f64>,
    pub win_rate: Option<f64>,
    pub profit_factor: Option<f64>,
    pub exposure: f64,
    pub turnover: f64,
    pub alpha: Option<f64>,
    pub beta: Option<f64>,
    pub trades: u32,
    pub sessions: u32,
}
