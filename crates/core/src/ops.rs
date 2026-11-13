use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::market::{DataSource, Dataset, Symbol, Timeframe};
use crate::time::Timestamp;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobOutcome {
    Ok,
    Partial,
    Noop,
    Waiting,
    Backoff,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct JobRun {
    pub id: i64,
    pub job: String,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    pub outcome: Option<JobOutcome>,
    pub rows: u64,
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FetchStatus {
    Ok,
    Empty,
    RateLimited,
    Rejected,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SourceFetch {
    pub source: DataSource,
    pub dataset: Dataset,
    pub symbol: Option<Symbol>,
    pub requested_from: Option<NaiveDate>,
    pub requested_to: Option<NaiveDate>,
    pub at: Timestamp,
    pub status: FetchStatus,
    pub http_status: Option<u16>,
    pub rows: u64,
    pub retry_after: Option<Timestamp>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CoverageGap {
    pub symbol: Symbol,
    pub timeframe: Timeframe,
    pub from: NaiveDate,
    pub to: NaiveDate,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DataWarning {
    CoverageGap(CoverageGap),
    UnexplainedMove {
        symbol: Symbol,
        date: NaiveDate,
        change: f64,
    },
    OverlapRejected {
        symbol: Symbol,
        source: DataSource,
        message: String,
    },
    ActionDisagreement(ActionDisagreement),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckedAction {
    Split,
    CashDividend,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActionVerdict {
    Alpaca,
    Tiingo,
    Prices,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ActionDisagreement {
    pub symbol: Symbol,
    pub ex_date: NaiveDate,
    pub action: CheckedAction,
    #[schema(value_type = Option<String>)]
    pub alpaca: Option<Decimal>,
    #[schema(value_type = Option<String>)]
    pub tiingo: Option<Decimal>,
    pub kept: ActionVerdict,
}

pub const UNEXPLAINED_MOVE_THRESHOLD: f64 = 0.40;
