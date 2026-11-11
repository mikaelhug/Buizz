use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use utoipa::{IntoParams, ToSchema};

use crate::market::{Coverage, Symbol, Timeframe, Watermarks};
use crate::ops::{DataWarning, JobOutcome};
use crate::order::{Fill, Order, OrderUpdate};
use crate::performance::Metrics;
use crate::run::{
    BotManifest, CostModel, DesiredState, EquityPoint, Mode, Period, PeriodPreset, Reproducibility,
    RiskLimits, RunBadge, RunConfig, RunEvent, RunId, RunStatus,
};
use crate::time::Timestamp;

pub const SESSION_COOKIE: &str = "buizz_session";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BotStatus {
    Idle,
    Live,
    Paused,
    Backtesting,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SparkPoint {
    pub at: Timestamp,
    #[serde(with = "crate::money::cash")]
    pub equity: Decimal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BotSummary {
    pub name: String,
    pub description: String,
    pub timeframe: Timeframe,
    pub status: BotStatus,
    pub live_run: Option<RunId>,
    #[serde(default, with = "crate::money::cash_option")]
    pub equity: Option<Decimal>,
    pub sparkline: Vec<SparkPoint>,
    pub stats: Option<Metrics>,
    pub stats_run: Option<RunId>,
    pub holdout_runs: u32,
    pub last_run_at: Option<Timestamp>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BotDetail {
    pub summary: BotSummary,
    pub manifest: BotManifest,
    pub bot_hash: String,
    pub runs: Vec<RunSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RunSummary {
    pub id: RunId,
    pub bot: String,
    pub mode: Mode,
    pub status: RunStatus,
    pub desired: DesiredState,
    pub config: RunConfig,
    pub created_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
    pub clock: Option<Timestamp>,
    #[serde(default, with = "crate::money::cash_option")]
    pub equity: Option<Decimal>,
    pub metrics: Option<Metrics>,
    pub result_hash: Option<String>,
    pub badges: Vec<RunBadge>,
    pub reproducibility: Option<Reproducibility>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EquitySeries {
    pub run_id: RunId,
    pub points: Vec<EquityPoint>,
    pub benchmark: Vec<SparkPoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct OrderView {
    #[serde(flatten)]
    pub order: Order,
    #[serde(default, with = "crate::money::price_option")]
    pub avg_fill_price: Option<Decimal>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct OrdersPage {
    pub items: Vec<OrderView>,
    pub next_after: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PositionView {
    pub symbol: Symbol,
    pub qty: i64,
    #[serde(with = "crate::money::price")]
    pub avg_cost: Decimal,
    #[serde(with = "crate::money::price")]
    pub last_price: Decimal,
    #[serde(with = "crate::money::cash")]
    pub market_value: Decimal,
    #[serde(with = "crate::money::cash")]
    pub unrealized_pnl: Decimal,
    #[serde(with = "crate::money::cash")]
    pub realized_pnl: Decimal,
    pub weight: Decimal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EventsPage {
    pub items: Vec<RunEvent>,
    pub next_after: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StartBacktest {
    pub preset: Option<PeriodPreset>,
    pub period: Option<Period>,
    #[schema(value_type = Option<Object>)]
    pub params: Option<Map<String, Value>>,
    pub start_cash: Option<Decimal>,
    pub cost: Option<CostModel>,
    pub risk: Option<RiskLimits>,
    pub seed: Option<u64>,
    #[serde(default)]
    pub allow_holdout: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LeaderboardRow {
    pub bot: String,
    pub run_id: RunId,
    pub metrics: Metrics,
    pub finished_at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Leaderboard {
    pub preset: PeriodPreset,
    pub period: Period,
    pub cost: CostModel,
    pub rows: Vec<LeaderboardRow>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DataHealth {
    pub watermarks: Watermarks,
    pub coverage: Vec<Coverage>,
    pub warnings: Vec<DataWarning>,
    pub stale_jobs: u32,
    pub ingest_enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct JobView {
    pub name: String,
    pub every: String,
    pub last_started: Option<Timestamp>,
    pub last_finished: Option<Timestamp>,
    pub last_outcome: Option<JobOutcome>,
    pub last_message: Option<String>,
    pub next_due: Option<Timestamp>,
    pub stale: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct FeedHolder {
    pub run_id: RunId,
    pub bot: String,
    pub symbols: Vec<Symbol>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct FeedSlots {
    pub cap: u32,
    pub used: u32,
    pub connected: bool,
    pub error: Option<String>,
    pub holders: Vec<FeedHolder>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub password: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SessionInfo {
    pub authenticated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ApiError {
    pub error: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SpineEvent {
    RunStatus {
        run_id: RunId,
        bot: String,
        status: RunStatus,
    },
    Equity {
        run_id: RunId,
        bot: String,
        at: Timestamp,
        #[serde(with = "crate::money::cash")]
        equity: Decimal,
    },
    Order {
        run_id: RunId,
        order: OrderUpdate,
    },
    Fill {
        run_id: RunId,
        fill: Fill,
    },
    Log {
        run_id: RunId,
        event: RunEvent,
    },
    Job {
        job: JobView,
    },
    Feed {
        slots: FeedSlots,
    },
}

impl SpineEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::RunStatus { .. } => "run_status",
            Self::Equity { .. } => "equity",
            Self::Order { .. } => "order",
            Self::Fill { .. } => "fill",
            Self::Log { .. } => "log",
            Self::Job { .. } => "job",
            Self::Feed { .. } => "feed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RunsQuery {
    pub bot: Option<String>,
    pub mode: Option<Mode>,
    pub before: Option<RunId>,
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PageQuery {
    pub after: Option<u64>,
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LeaderboardQuery {
    pub preset: PeriodPreset,
}
