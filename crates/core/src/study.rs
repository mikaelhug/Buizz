use chrono::NaiveDate;
use rust_decimal::Decimal;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::market::{Liquidity, Symbol};
use crate::money::round_price;
use crate::run::CostModel;
use crate::time::Timestamp;

pub const STUDY_HORIZONS: [u32; 4] = [1, 5, 21, 63];
pub const STUDY_SESSIONS: u32 = STUDY_HORIZONS[STUDY_HORIZONS.len() - 1];
pub const STUDY_POSITION: Decimal = Decimal::from_parts(10_000, 0, 0, false, 0);
pub const INDEX_BENCHMARK: &str = "SPY";
pub const BROAD_BENCHMARK: &str = "IWM";
pub const SESSIONS_PER_MONTH: u32 = 21;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StudyEvent {
    pub symbol: Symbol,
    pub at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StudyRequest {
    pub label: String,
    pub events: Vec<StudyEvent>,
    pub benchmark: Option<Symbol>,
    pub cost: Option<CostModel>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StudySkip {
    NoSession,
    UnknownSymbol,
    NotTraded,
    BenchmarkNotTraded,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HorizonReturn {
    pub stock: f64,
    pub benchmark: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EventOutcome {
    pub symbol: Symbol,
    pub at: Timestamp,
    pub entry: Option<NaiveDate>,
    pub benchmark: Option<Symbol>,
    pub round_trip_cost: Option<f64>,
    pub last_session: Option<NaiveDate>,
    pub skipped: Option<StudySkip>,
    pub returns: Vec<Option<HorizonReturn>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AbnormalStats {
    pub mean: Option<f64>,
    pub median: Option<f64>,
    pub hit_rate: Option<f64>,
    pub t_stat: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HorizonStats {
    pub sessions: u32,
    pub count: u32,
    pub delisted: u32,
    pub months: u32,
    pub gross: AbnormalStats,
    pub net: AbnormalStats,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CurvePoint {
    pub session: u32,
    pub count: u32,
    pub gross: f64,
    pub net: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EventStudy {
    pub label: String,
    pub hash: String,
    pub trials: u32,
    pub luck_t: f64,
    pub data_through: Option<Timestamp>,
    pub cost: CostModel,
    pub position: Decimal,
    pub horizons: Vec<HorizonStats>,
    pub curve: Vec<CurvePoint>,
    pub events: Vec<EventOutcome>,
}

pub fn round_trip_cost(cost: &CostModel, open: f64, liquidity: Option<&Liquidity>) -> Option<f64> {
    let price = Decimal::from_f64(open)
        .map(round_price)
        .filter(|price| *price > Decimal::ZERO)?;
    let shares = (STUDY_POSITION / price).floor().max(Decimal::ONE);
    let qty = shares.to_u64()?;
    let commission = cost.commission(qty, 0) / (shares * price);
    let one_way = cost.slippage(liquidity, 0, qty) + cost.slippage_per_share / price + commission;
    (one_way * Decimal::TWO).to_f64()
}

pub fn overlap_months(sessions: u32) -> u32 {
    sessions.saturating_sub(1).div_ceil(SESSIONS_PER_MONTH)
}
