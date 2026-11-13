use std::collections::BTreeMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use utoipa::{IntoParams, ToSchema};

use crate::market::{Coverage, DataSource, Dataset, Symbol, Timeframe, Watermarks};
use crate::ops::{DataWarning, JobOutcome};
use crate::order::{Fill, Order, OrderId, OrderUpdate};
use crate::performance::{LivePoint, LiveStats, Metrics, RoundTrip};
use crate::run::{
    BotManifest, CostModel, DesiredState, EquityPoint, Mode, Period, PeriodPreset, Reproducibility,
    RiskLimits, RunBadge, RunConfig, RunEvent, RunEventType, RunId, RunStatus,
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
    Stopped,
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
    #[schema(value_type = Object)]
    pub params: Map<String, Value>,
    pub stage: BotStage,
    pub screening: Option<Screening>,
    pub backtest: Option<BacktestGlance>,
    pub live: Vec<LiveSummary>,
    pub last_run_at: Option<Timestamp>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BacktestGlance {
    pub run_id: RunId,
    pub preset: Option<PeriodPreset>,
    pub period: Period,
    pub plateau: bool,
    pub failed_checks: Vec<GateCheck>,
    pub sub_periods: Vec<f64>,
    #[schema(value_type = Object)]
    pub params: Map<String, Value>,
    pub metrics: Metrics,
    pub benchmark: Metrics,
    pub curve: Vec<LivePoint>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Screening {
    pub preset: PeriodPreset,
    pub variants: u32,
    pub passing: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Trials {
    pub variants: u32,
    pub params: u32,
    pub runs: u32,
    pub unversioned_runs: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GateCheck {
    NotCompleted,
    DataWarnings,
    DrawdownStop,
    FewTrades,
    NoEdge,
    TrailsBenchmark,
    DeeperDrawdown,
    InconsistentEdge,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct VariantResult {
    pub run_id: RunId,
    pub status: RunStatus,
    pub period: Period,
    #[schema(value_type = Object)]
    pub params: Map<String, Value>,
    pub metrics: Option<Metrics>,
    pub badges: Vec<RunBadge>,
    pub failed_checks: Vec<GateCheck>,
    pub sub_periods: Vec<f64>,
    pub excess_cagr: Option<f64>,
    pub neighbourhood_sharpe: Option<f64>,
    pub curve: Vec<LivePoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PresetResearch {
    pub preset: PeriodPreset,
    pub benchmark_sharpe: Option<f64>,
    pub benchmark_max_drawdown: Option<f64>,
    pub luck_sharpe: Option<f64>,
    pub plateau: Option<RunId>,
    pub variants: Vec<VariantResult>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Research {
    pub trials: Trials,
    pub gate_preset: Option<PeriodPreset>,
    pub presets: Vec<PresetResearch>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StartSweep {
    pub preset: PeriodPreset,
    pub grid: BTreeMap<String, Vec<Value>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SweepStarted {
    pub period: Period,
    pub queued: Vec<RunId>,
    pub skipped: u32,
    pub trials: Trials,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CancelledBacktests {
    pub cancelled: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GoLive {
    pub from_run: Option<RunId>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelRun {
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BotStage {
    Draft,
    Screened,
    Live,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LiveSummary {
    pub run: RunSummary,
    pub sessions_needed: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct League {
    pub started: u32,
    pub ranked: Vec<LiveSummary>,
    pub too_early: Vec<LiveSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BotDetail {
    pub summary: BotSummary,
    pub status: BotStatus,
    pub holdout_runs: u32,
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
    pub live_stats: Option<LiveStats>,
    pub result_hash: Option<String>,
    pub badges: Vec<RunBadge>,
    pub reproducibility: Option<Reproducibility>,
    pub message: Option<String>,
    pub usage: BotUsage,
    pub source_run: Option<RunId>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BotUsage {
    pub processes: u32,
    pub seconds: f64,
    pub peak_memory: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ProcessSlots {
    pub backtest: u32,
    pub live: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BotFootprint {
    pub bot: String,
    pub mode: Mode,
    pub runs: u32,
    pub usage: BotUsage,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BotProcesses {
    pub slots: ProcessSlots,
    pub footprints: Vec<BotFootprint>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EquitySeries {
    pub run_id: RunId,
    pub points: Vec<EquityPoint>,
    pub benchmark: Vec<SparkPoint>,
    pub benchmark_metrics: Metrics,
    pub interim_metrics: Option<Metrics>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct OrderView {
    #[serde(flatten)]
    pub order: Order,
    #[serde(default, with = "crate::money::price_option")]
    pub avg_fill_price: Option<Decimal>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TradeView {
    #[serde(flatten)]
    pub trip: RoundTrip,
    pub opening_reason: Option<String>,
    pub closing_reason: Option<String>,
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
pub struct PriceChart {
    pub symbol: Symbol,
    pub timeframe: Timeframe,
    pub bars: Vec<PriceBar>,
    pub older: bool,
    pub fills: Vec<ChartFill>,
    pub splits: Vec<ChartSplit>,
    pub position: Option<ChartPosition>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PriceBar {
    pub start: Timestamp,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChartFill {
    pub order_id: OrderId,
    pub at: Timestamp,
    pub qty: i64,
    #[serde(with = "crate::money::price")]
    pub price: Decimal,
    #[serde(with = "crate::money::price")]
    pub chart_price: Decimal,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChartSplit {
    pub ex_date: NaiveDate,
    pub ratio: Decimal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChartPosition {
    pub qty: i64,
    #[serde(with = "crate::money::price")]
    pub avg_cost: Decimal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EventsPage {
    pub items: Vec<RunEvent>,
    pub next_after: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct BackfillProgress {
    pub dataset: Dataset,
    pub source: DataSource,
    pub done: u32,
    pub total: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DataHealth {
    pub watermarks: Watermarks,
    pub backfill: Vec<BackfillProgress>,
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
pub struct FeedConnection {
    pub cap: u32,
    pub used: u32,
    pub connected: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct FeedSlots {
    pub cap: u32,
    pub used: u32,
    pub error: Option<String>,
    pub connections: Vec<FeedConnection>,
    pub holders: Vec<FeedHolder>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub username: String,
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
        mode: Mode,
        status: RunStatus,
    },
    LiveSession {
        run_id: RunId,
        bot: String,
        summary: Box<LiveSummary>,
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
    Usage {
        run_id: RunId,
        usage: BotUsage,
    },
}

impl SpineEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::RunStatus { .. } => "run_status",
            Self::LiveSession { .. } => "live_session",
            Self::Equity { .. } => "equity",
            Self::Order { .. } => "order",
            Self::Fill { .. } => "fill",
            Self::Log { .. } => "log",
            Self::Job { .. } => "job",
            Self::Feed { .. } => "feed",
            Self::Usage { .. } => "usage",
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
pub struct EventsQuery {
    pub after: Option<u64>,
    pub limit: Option<u32>,
    #[serde(default)]
    #[param(inline)]
    pub kind: Vec<RunEventType>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ChartQuery {
    pub after: Option<Timestamp>,
    pub before: Option<Timestamp>,
}
