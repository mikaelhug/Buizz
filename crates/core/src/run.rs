use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use chrono::{Months, NaiveDate, NaiveTime, TimeDelta};
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use rust_decimal::{Decimal, MathematicalOps};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use utoipa::openapi::schema::{ObjectBuilder, Type};
use utoipa::openapi::{RefOr, Schema};
use utoipa::{PartialSchema, ToSchema};

use crate::market::{AssetClass, Bar, Liquidity, Session, StockIndex, Symbol, Timeframe};
use crate::money::{PRICE_DECIMALS, round_cash, round_price};
use crate::order::{RejectReason, Side};
use crate::time::Timestamp;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(transparent)]
pub struct RunId(pub i64);

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Backtest,
    Live,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum UniverseSpec {
    Symbols {
        symbols: Vec<Symbol>,
    },
    TopDollarVolume {
        top_dollar_volume: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        asset_class: Option<AssetClass>,
    },
    Index {
        index: StockIndex,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Wake {
    #[default]
    Bar,
    Every {
        minutes: u32,
    },
    At {
        time: NaiveTime,
    },
    BeforeClose {
        minutes: u32,
    },
}

impl Wake {
    pub fn before_close(&self, session: &Session) -> Option<Timestamp> {
        let Self::BeforeClose { minutes } = *self else {
            return None;
        };
        let at = session.close - TimeDelta::minutes(i64::from(minutes));
        (at > session.open).then_some(at)
    }

    pub fn is_due(&self, clock: Timestamp, session: &Session) -> bool {
        match *self {
            Self::Bar => true,
            Self::Every { minutes } => {
                let elapsed = (clock - session.open).num_minutes();
                clock > session.open
                    && clock <= session.close
                    && minutes > 0
                    && elapsed % i64::from(minutes) == 0
            }
            Self::At { time } => clock <= session.close && clock.exchange_time() == time,
            Self::BeforeClose { .. } => self.before_close(session) == Some(clock),
        }
    }
}

impl fmt::Display for Wake {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bar => f.write_str("bar"),
            Self::Every { minutes } if minutes % 60 == 0 => write!(f, "every:{}h", minutes / 60),
            Self::Every { minutes } => write!(f, "every:{minutes}m"),
            Self::At { time } => write!(f, "at:{}", time.format("%H:%M")),
            Self::BeforeClose { minutes } => write!(f, "before_close:{minutes}m"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid wake {0:?}, expected bar, every:<n>m, every:<n>h, at:HH:MM or before_close:<n>m")]
pub struct ParseWakeError(String);

impl FromStr for Wake {
    type Err = ParseWakeError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || ParseWakeError(text.to_owned());
        if text == "bar" {
            return Ok(Self::Bar);
        }
        if let Some(every) = text.strip_prefix("every:") {
            let (amount, scale) = match every.strip_suffix('h') {
                Some(hours) => (hours, 60),
                None => (every.strip_suffix('m').ok_or_else(invalid)?, 1),
            };
            let amount: u32 = amount.parse().map_err(|_| invalid())?;
            return match amount.checked_mul(scale) {
                Some(minutes) if minutes > 0 => Ok(Self::Every { minutes }),
                _ => Err(invalid()),
            };
        }
        if let Some(before) = text.strip_prefix("before_close:") {
            let minutes: u32 = before
                .strip_suffix('m')
                .and_then(|minutes| minutes.parse().ok())
                .filter(|minutes| *minutes > 0)
                .ok_or_else(invalid)?;
            return Ok(Self::BeforeClose { minutes });
        }
        if let Some(at) = text.strip_prefix("at:") {
            let time = NaiveTime::parse_from_str(at, "%H:%M").map_err(|_| invalid())?;
            return Ok(Self::At { time });
        }
        Err(invalid())
    }
}

impl Serialize for Wake {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl PartialSchema for Wake {
    fn schema() -> RefOr<Schema> {
        ObjectBuilder::new()
            .schema_type(Type::String)
            .pattern(Some(
                r"^(bar|every:[0-9]+[mh]|at:[0-2][0-9]:[0-5][0-9]|before_close:[0-9]+m)$",
            ))
            .examples(["bar", "every:30m", "at:15:55", "before_close:10m"])
            .into()
    }
}

impl ToSchema for Wake {}

impl<'de> Deserialize<'de> for Wake {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct CostModel {
    pub commission_per_share: Decimal,
    pub commission_min: Decimal,
    pub commission_tiers: Vec<CommissionTier>,
    pub slippage_bps: Decimal,
    pub slippage_per_share: Decimal,
    pub spread_scale: Decimal,
    pub impact_coefficient: Decimal,
    pub adv_participation: Decimal,
    pub adv_sessions: u32,
    pub borrow_rate: Decimal,
    pub financing_spread: Decimal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CommissionTier {
    pub above: u64,
    pub per_share: Decimal,
}

impl Default for CostModel {
    fn default() -> Self {
        Self {
            commission_per_share: Decimal::new(5, 3),
            commission_min: Decimal::ONE,
            commission_tiers: Vec::new(),
            slippage_bps: Decimal::new(5, 0),
            slippage_per_share: Decimal::ZERO,
            spread_scale: Decimal::ONE,
            impact_coefficient: Decimal::ONE,
            adv_participation: Decimal::new(5, 2),
            adv_sessions: 20,
            borrow_rate: Decimal::new(2, 2),
            financing_spread: Decimal::new(15, 3),
        }
    }
}

impl CostModel {
    pub const TRADING_DAYS_PER_YEAR: u32 = 252;

    pub fn invalid(&self) -> Option<&'static str> {
        let amounts = [
            self.commission_per_share,
            self.commission_min,
            self.slippage_bps,
            self.slippage_per_share,
            self.spread_scale,
            self.impact_coefficient,
            self.borrow_rate,
            self.financing_spread,
        ];
        if amounts.iter().any(|amount| *amount < Decimal::ZERO) {
            return Some("costs must not be negative");
        }
        if self
            .commission_tiers
            .iter()
            .any(|tier| tier.per_share < Decimal::ZERO)
        {
            return Some("commission tier prices must not be negative");
        }
        if self.adv_participation <= Decimal::ZERO || self.adv_participation > Decimal::ONE {
            return Some("adv_participation must be above 0 and at most 1");
        }
        if self.adv_sessions == 0 {
            return Some("adv_sessions must be at least 1");
        }
        None
    }

    pub fn slippage(&self, liquidity: Option<&Liquidity>, traded: u64, qty: u64) -> Decimal {
        let floor = self.slippage_bps / Decimal::new(10_000, 0);
        let Some(liquidity) = liquidity else {
            return floor;
        };
        let spread = floor.max(self.spread_scale * Self::half_spread(liquidity));
        spread
            + self.impact_coefficient
                * liquidity.volatility
                * marginal_impact(liquidity.average_daily_volume, traded, qty)
    }

    pub fn half_spread(liquidity: &Liquidity) -> Decimal {
        if liquidity.dollar_volume <= Decimal::ZERO {
            return Decimal::ZERO;
        }
        let relative_volatility = liquidity.volatility / INVARIANCE_VOLATILITY;
        let activity = relative_volatility * relative_volatility * INVARIANCE_DOLLAR_VOLUME
            / liquidity.dollar_volume;
        let root = activity
            .to_f64()
            .map(libm::cbrt)
            .and_then(Decimal::from_f64)
            .unwrap_or_default()
            .round_dp(Liquidity::VOLATILITY_DECIMALS);
        INVARIANCE_HALF_SPREAD * root
    }

    pub fn market_fill_price(&self, side: Side, price: Decimal, slippage: Decimal) -> Decimal {
        let moved = match side {
            Side::Buy => price * (Decimal::ONE + slippage) + self.slippage_per_share,
            Side::Sell => price * (Decimal::ONE - slippage) - self.slippage_per_share,
        };
        round_price(moved).max(Decimal::new(1, PRICE_DECIMALS))
    }

    pub fn stop_fill_price(
        &self,
        side: Side,
        stop: Decimal,
        bar: &Bar,
        slippage: Decimal,
    ) -> Option<Decimal> {
        let reference = match side {
            Side::Sell if bar.open <= stop => bar.open,
            Side::Sell if bar.low <= stop => stop,
            Side::Buy if bar.open >= stop => bar.open,
            Side::Buy if bar.high >= stop => stop,
            _ => return None,
        };
        Some(self.market_fill_price(side, reference, slippage))
    }

    pub fn limit_fill_price(
        &self,
        side: Side,
        limit: Decimal,
        bar: &Bar,
        slippage: Decimal,
    ) -> Option<Decimal> {
        let at_open = self.market_fill_price(side, bar.open, slippage);
        match side {
            Side::Buy if bar.open <= limit => Some(at_open.min(limit)),
            Side::Buy if bar.low <= limit => Some(limit),
            Side::Sell if bar.open >= limit => Some(at_open.max(limit)),
            Side::Sell if bar.high >= limit => Some(limit),
            _ => None,
        }
    }

    pub fn commission(&self, qty: u64, shares_this_month: u64) -> Decimal {
        if qty == 0 {
            return Decimal::ZERO;
        }
        let per_share = self
            .commission_tiers
            .iter()
            .filter(|tier| shares_this_month >= tier.above)
            .max_by_key(|tier| tier.above)
            .map_or(self.commission_per_share, |tier| tier.per_share);
        round_cash((per_share * Decimal::from(qty)).max(self.commission_min))
    }

    pub fn participation_cap(&self, average_daily_volume: Option<u64>) -> Option<u64> {
        average_daily_volume.map(|volume| {
            (Decimal::from(volume) * self.adv_participation)
                .floor()
                .to_u64()
                .unwrap_or(0)
        })
    }

    pub fn borrow_fee(&self, short_qty: u64, price: Decimal) -> Decimal {
        round_cash(
            Decimal::from(short_qty) * price * self.borrow_rate
                / Decimal::from(Self::TRADING_DAYS_PER_YEAR),
        )
    }

    pub fn financing_rate(&self, risk_free_rate: Decimal) -> Decimal {
        risk_free_rate + self.financing_spread
    }

    pub fn financing_fee(&self, cash: Decimal, rate: Decimal) -> Decimal {
        if cash >= Decimal::ZERO {
            return Decimal::ZERO;
        }
        round_cash(-cash * rate / Decimal::from(Self::TRADING_DAYS_PER_YEAR))
    }
}

const INVARIANCE_HALF_SPREAD: Decimal = Decimal::from_parts(821, 0, 0, false, 6);
const INVARIANCE_VOLATILITY: Decimal = Decimal::from_parts(2, 0, 0, false, 2);
const INVARIANCE_DOLLAR_VOLUME: Decimal = Decimal::from_parts(40_000_000, 0, 0, false, 0);

fn marginal_impact(average_daily_volume: u64, traded: u64, qty: u64) -> Decimal {
    if average_daily_volume == 0 || qty == 0 {
        return Decimal::ZERO;
    }
    let volume = Decimal::from(average_daily_volume);
    let before = Decimal::from(traded) / volume;
    let after = Decimal::from(traded.saturating_add(qty)) / volume;
    let cost = |fraction: Decimal| fraction * fraction.sqrt().unwrap_or_default();
    (cost(after) - cost(before)) / (after - before)
}

pub const MOC_CUTOFF_MINUTES: i64 = 10;

pub const RATE_LOOKBACK_DAYS: i64 = 31;

pub const HALF_DAY_MINUTES: u32 = 210;

pub const UNIVERSE_SESSIONS: u32 = 20;

pub const DRAWDOWN_STOP: &str = "stopped by drawdown limit";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RiskLimits {
    pub max_position_weight: Option<Decimal>,
    pub max_gross_exposure: Option<Decimal>,
    pub max_drawdown: Option<Decimal>,
}

impl RiskLimits {
    pub fn are_positive(&self) -> bool {
        [
            self.max_position_weight,
            self.max_gross_exposure,
            self.max_drawdown,
        ]
        .into_iter()
        .flatten()
        .all(|limit| limit > Decimal::ZERO)
    }

    pub fn check_order(
        &self,
        equity: Decimal,
        position_exposure_after: Decimal,
        gross_exposure_after: Decimal,
    ) -> Result<(), RejectReason> {
        let exceeds = |exposure: Decimal, limit: Decimal| {
            equity <= Decimal::ZERO || exposure / equity > limit
        };
        if self
            .max_position_weight
            .is_some_and(|limit| exceeds(position_exposure_after.abs(), limit))
        {
            return Err(RejectReason::MaxPositionWeight);
        }
        if self
            .max_gross_exposure
            .is_some_and(|limit| exceeds(gross_exposure_after, limit))
        {
            return Err(RejectReason::MaxGrossExposure);
        }
        Ok(())
    }

    pub fn drawdown_breached(&self, peak_equity: Decimal, equity: Decimal) -> bool {
        match self.max_drawdown {
            Some(limit) if peak_equity > Decimal::ZERO => {
                (peak_equity - equity) / peak_equity >= limit
            }
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RunConfig {
    pub bot: String,
    pub mode: Mode,
    #[schema(value_type = Object)]
    pub params: Map<String, Value>,
    pub universe: UniverseSpec,
    pub timeframe: Timeframe,
    pub wake: Wake,
    pub start: NaiveDate,
    pub end: Option<NaiveDate>,
    pub start_cash: Decimal,
    pub cost: CostModel,
    pub risk: RiskLimits,
    pub seed: u64,
    pub benchmark: Symbol,
    pub holdout: bool,
}

impl RunConfig {
    pub fn needs_minute_bars(&self) -> bool {
        self.timeframe == Timeframe::Minute || matches!(self.wake, Wake::BeforeClose { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Starting,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub const ACTIVE: [Self; 4] = [Self::Queued, Self::Starting, Self::Running, Self::Paused];

    pub fn is_finished(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DesiredState {
    Running,
    Paused,
    Stopped,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
pub enum PeriodPreset {
    #[serde(rename = "1y")]
    OneYear,
    #[serde(rename = "3y")]
    ThreeYears,
    #[serde(rename = "5y")]
    FiveYears,
    #[serde(rename = "10y")]
    TenYears,
    #[serde(rename = "holdout")]
    Holdout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
pub struct Period {
    pub from: NaiveDate,
    pub to: NaiveDate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PresetPeriod {
    pub preset: PeriodPreset,
    pub period: Period,
    pub shortened: bool,
}

pub const HOLDOUT_MONTHS: u32 = 12;

pub const PRESET_FLOOR: NaiveDate = NaiveDate::from_ymd_opt(2016, 1, 4).expect("valid date");

pub const MAX_LIVE_VARIANTS: u32 = 3;

pub const MAX_SWEEP_VARIANTS: usize = 256;

pub fn coerce_param(default: &Value, requested: Value) -> Result<Value, String> {
    let coerced = match (default, &requested) {
        (Value::Null, _) => Some(requested.clone()),
        (Value::Number(number), Value::Number(value)) if number.is_f64() => {
            value.as_f64().map(Value::from)
        }
        (Value::Number(_), Value::Number(value)) => value.as_i64().map(Value::from).or_else(|| {
            value
                .as_f64()
                .filter(|whole| whole.fract() == 0.0 && whole.abs() < 2f64.powi(53))
                .map(|whole| Value::from(whole as i64))
        }),
        (Value::Bool(_), Value::Bool(_))
        | (Value::String(_), Value::String(_))
        | (Value::Array(_), Value::Array(_))
        | (Value::Object(_), Value::Object(_)) => Some(requested.clone()),
        _ => None,
    };
    coerced.ok_or_else(|| {
        let kind = match default {
            Value::Number(number) if number.is_f64() => "a number",
            Value::Number(_) => "a whole number",
            Value::Bool(_) => "true or false",
            Value::String(_) => "text",
            Value::Array(_) => "a list",
            _ => "an object",
        };
        format!("must be {kind} like its default {default}, got {requested}")
    })
}

pub fn holdout_start(today: NaiveDate) -> NaiveDate {
    today
        .checked_sub_months(Months::new(HOLDOUT_MONTHS))
        .unwrap_or(NaiveDate::MIN)
}

impl Period {
    pub fn touches_holdout(&self, today: NaiveDate) -> bool {
        self.to >= holdout_start(today)
    }
}

impl PeriodPreset {
    pub const SCREENING: [Self; 4] = [
        Self::TenYears,
        Self::FiveYears,
        Self::ThreeYears,
        Self::OneYear,
    ];

    pub fn of(period: Period, created_on: NaiveDate) -> Option<Self> {
        Self::SCREENING
            .into_iter()
            .find(|preset| preset.resolve(created_on) == period)
    }

    fn unfloored_start(self, today: NaiveDate) -> NaiveDate {
        let holdout = holdout_start(today);
        let years = match self {
            Self::Holdout => return holdout,
            Self::OneYear => 1,
            Self::ThreeYears => 3,
            Self::FiveYears => 5,
            Self::TenYears => 10,
        };
        holdout
            .checked_sub_months(Months::new(12 * years))
            .unwrap_or(NaiveDate::MIN)
    }

    pub fn resolve(self, today: NaiveDate) -> Period {
        let holdout = holdout_start(today);
        match self {
            Self::Holdout => Period {
                from: holdout,
                to: today,
            },
            _ => Period {
                from: self.unfloored_start(today).max(PRESET_FLOOR),
                to: holdout - TimeDelta::days(1),
            },
        }
    }

    pub fn described(self, today: NaiveDate) -> PresetPeriod {
        let period = self.resolve(today);
        PresetPeriod {
            preset: self,
            period,
            shortened: self.unfloored_start(today) < period.from,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EquityPoint {
    pub at: Timestamp,
    #[serde(with = "crate::money::cash")]
    pub equity: Decimal,
    #[serde(with = "crate::money::cash")]
    pub cash: Decimal,
    #[serde(with = "crate::money::cash")]
    pub gross_exposure: Decimal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RiskLimit {
    MaxPositionWeight,
    MaxGrossExposure,
    MaxDrawdown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEventKind {
    Log {
        level: LogLevel,
        message: String,
    },
    Status {
        status: RunStatus,
        message: Option<String>,
    },
    OrderRejected {
        client_id: String,
        symbol: Symbol,
        reason: RejectReason,
    },
    RiskBreach {
        limit: RiskLimit,
        value: Decimal,
        threshold: Decimal,
    },
    BotExited {
        code: Option<i32>,
    },
    DataWarning {
        message: String,
    },
}

impl RunEventKind {
    pub fn event_type(&self) -> RunEventType {
        match self {
            Self::Log { .. } => RunEventType::Log,
            Self::Status { .. } => RunEventType::Status,
            Self::OrderRejected { .. } => RunEventType::OrderRejected,
            Self::RiskBreach { .. } => RunEventType::RiskBreach,
            Self::BotExited { .. } => RunEventType::BotExited,
            Self::DataWarning { .. } => RunEventType::DataWarning,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunEventType {
    Log,
    Status,
    OrderRejected,
    RiskBreach,
    BotExited,
    DataWarning,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RunEvent {
    pub seq: u64,
    pub at: Timestamp,
    pub clock: Option<Timestamp>,
    pub kind: RunEventKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Reproducibility {
    pub bot_hash: String,
    pub behaviour_hash: Option<String>,
    pub engine_version: String,
    pub seed: u64,
    pub data_watermark: Option<Timestamp>,
}

impl Reproducibility {
    pub fn code(&self) -> BotCode {
        BotCode {
            content_hash: self.bot_hash.clone(),
            behaviour_hash: self.behaviour_hash.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BotCode {
    pub content_hash: String,
    pub behaviour_hash: Option<String>,
}

impl BotCode {
    pub fn same_as(&self, other: &BotCode) -> bool {
        match (&self.behaviour_hash, &other.behaviour_hash) {
            (Some(own), Some(theirs)) => own == theirs,
            _ => self.content_hash == other.content_hash,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunBadge {
    SurvivorshipBiased,
    Holdout,
    DataWarnings,
    DrawdownStop,
    ThinIndexHistory,
    MinorDataGaps,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BotManifest {
    #[serde(default)]
    pub description: String,
    pub command: Vec<String>,
    pub timeframe: Timeframe,
    #[serde(default)]
    pub wake: Wake,
    pub universe: UniverseSpec,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub params: Map<String, Value>,
    #[serde(default = "BotManifest::default_benchmark")]
    pub benchmark: Symbol,
    #[serde(default)]
    pub risk: RiskLimits,
    #[serde(default = "BotManifest::default_step_timeout")]
    pub step_timeout: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("bot.toml is not valid: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("bot.toml is not valid: {0}")]
    Invalid(&'static str),
}

impl BotManifest {
    pub const FILE_NAME: &'static str = "bot.toml";

    pub const MAX_STEP_TIMEOUT: u32 = 3600;

    fn default_benchmark() -> Symbol {
        Symbol::new("SPY")
    }

    fn default_step_timeout() -> u32 {
        60
    }

    pub fn step_timeout(&self) -> Duration {
        Duration::from_secs(u64::from(self.step_timeout))
    }

    pub fn from_toml(text: &str) -> Result<Self, ManifestError> {
        let manifest: Self = toml::from_str(text)?;
        if manifest.command.is_empty() {
            return Err(ManifestError::Invalid("command must not be empty"));
        }
        if manifest.timeframe == Timeframe::Day
            && !matches!(manifest.wake, Wake::Bar | Wake::BeforeClose { .. })
        {
            return Err(ManifestError::Invalid(
                "daily bots must use wake = \"bar\" or \"before_close:<n>m\"",
            ));
        }
        if !manifest.risk.are_positive() {
            return Err(ManifestError::Invalid("risk limits must be positive"));
        }
        if !(1..=Self::MAX_STEP_TIMEOUT).contains(&manifest.step_timeout) {
            return Err(ManifestError::Invalid(
                "step_timeout must be between 1 and 3600 seconds",
            ));
        }
        if let Wake::BeforeClose { minutes } = manifest.wake
            && minutes >= HALF_DAY_MINUTES
        {
            return Err(ManifestError::Invalid(
                "before_close must be under 210 minutes, the length of a half-day session",
            ));
        }

        match &manifest.universe {
            UniverseSpec::Symbols { symbols } if symbols.is_empty() => {
                Err(ManifestError::Invalid("universe.symbols must not be empty"))
            }
            UniverseSpec::TopDollarVolume {
                top_dollar_volume: 0,
                ..
            } => Err(ManifestError::Invalid(
                "universe.top_dollar_volume must be positive",
            )),
            _ => Ok(manifest),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::Feed;

    #[test]
    fn an_illiquid_stock_pays_its_estimated_spread_and_impact_and_a_liquid_one_the_floor() {
        let window = |closes: [&str; 4], volume: u64| -> Vec<Bar> {
            closes
                .iter()
                .map(|close| Bar {
                    symbol: Symbol::new("X"),
                    start: Timestamp::MIN,
                    open: close.parse().unwrap(),
                    high: close.parse().unwrap(),
                    low: close.parse().unwrap(),
                    close: close.parse().unwrap(),
                    volume,
                    feed: Feed::Sip,
                })
                .collect()
        };
        let decimal = |text: &str| text.parse::<Decimal>().unwrap();
        let liquid = Liquidity::estimate(&window(["100", "101", "100", "100"], 10_000_000), &[]);
        let illiquid = Liquidity::estimate(&window(["10", "10.2", "9.996", "9.996"], 50_000), &[]);
        let (liquid, illiquid) = (liquid.unwrap(), illiquid.unwrap());
        assert_eq!(
            (liquid.dollar_volume, liquid.volatility),
            (decimal("1002500000"), decimal("0.00995054"))
        );
        assert_eq!(
            (illiquid.dollar_volume, illiquid.volatility),
            (decimal("502400"), decimal("0.02"))
        );
        assert_eq!(CostModel::half_spread(&liquid), decimal("0.00017614871085"));
        assert_eq!(
            CostModel::half_spread(&illiquid),
            decimal("0.00353193967657")
        );
        let cost = CostModel::default();
        let liquid_slippage = cost.slippage(Some(&liquid), 0, 100);
        let illiquid_slippage = cost.slippage(Some(&illiquid), 0, 500);
        assert_eq!(liquid_slippage.round_dp(12), decimal("0.000531466370"));
        assert_eq!(illiquid_slippage, decimal("0.00553193967657"));
        assert_eq!(
            cost.market_fill_price(Side::Buy, Decimal::from(100), liquid_slippage),
            decimal("100.0531")
        );
        assert_eq!(
            cost.market_fill_price(Side::Buy, Decimal::from(10), illiquid_slippage),
            decimal("10.0553")
        );
    }

    #[test]
    fn a_pre_close_wake_that_would_skip_half_days_is_refused() {
        let manifest = |wake: &str| {
            BotManifest::from_toml(&format!(
                "command = [\"true\"]\ntimeframe = \"1d\"\nwake = \"{wake}\"\nuniverse = {{ symbols = [\"SPY\"] }}\n"
            ))
        };
        assert!(manifest("before_close:209m").is_ok());
        assert!(manifest("before_close:210m").is_err());
    }

    #[test]
    fn without_limits_no_order_is_rejected_for_exposure_even_at_negative_equity() {
        let exposure = Decimal::from(1000);
        let equity = Decimal::from(-5);
        assert_eq!(
            RiskLimits::default().check_order(equity, exposure, exposure),
            Ok(())
        );
    }

    #[test]
    fn a_screening_preset_never_starts_before_the_daily_history_floor() {
        let today: NaiveDate = "2026-11-13".parse().unwrap();
        let ten = PeriodPreset::TenYears.described(today);
        assert_eq!(ten.period.from, PRESET_FLOOR);
        assert!(ten.shortened);
        assert_eq!(
            PeriodPreset::of(ten.period, today),
            Some(PeriodPreset::TenYears)
        );
        let before_the_floor = Period {
            from: "2015-11-13".parse().unwrap(),
            to: ten.period.to,
        };
        assert_eq!(PeriodPreset::of(before_the_floor, today), None);
        assert!(!PeriodPreset::FiveYears.described(today).shortened);
    }
}
