use chrono::NaiveDate;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::market::{Feed, Symbol};
use crate::time::Timestamp;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(transparent)]
pub struct OrderId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn of_qty(qty: i64) -> Self {
        if qty < 0 { Self::Sell } else { Self::Buy }
    }

    pub fn signed(self, qty: u64) -> i64 {
        let qty = i64::try_from(qty).unwrap_or(i64::MAX);
        match self {
            Self::Buy => qty,
            Self::Sell => -qty,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OrderType {
    Market,
    Limit { limit_price: Decimal },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimeInForce {
    #[default]
    Day,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum OrderSize {
    Qty { qty: i64 },
    TargetWeight { weight: Decimal },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct OrderRequest {
    pub client_id: String,
    pub symbol: Symbol,
    pub size: OrderSize,
    pub order_type: OrderType,
    #[serde(default)]
    pub tif: TimeInForce,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    Working,
    Filled,
    Cancelled,
    Expired,
    Rejected,
}

impl OrderStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Working)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    UnknownSymbol,
    DuplicateClientId,
    ZeroQuantity,
    InvalidLimitPrice,
    NoPrice,
    NotShortable,
    MaxPositionWeight,
    MaxGrossExposure,
    InvalidSize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Order {
    pub id: OrderId,
    pub client_id: String,
    pub symbol: Symbol,
    pub side: Side,
    pub qty: u64,
    pub filled_qty: u64,
    pub requested: OrderSize,
    pub order_type: OrderType,
    pub tif: TimeInForce,
    pub reason: String,
    pub decided_at: Timestamp,
    pub session: Option<NaiveDate>,
    pub status: OrderStatus,
    pub reject_reason: Option<RejectReason>,
    pub updated_at: Timestamp,
}

impl Order {
    pub fn remaining(&self) -> u64 {
        self.qty.saturating_sub(self.filled_qty)
    }

    pub fn signed_remaining(&self) -> i64 {
        if self.status.is_terminal() {
            0
        } else {
            self.side.signed(self.remaining())
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct OrderUpdate {
    pub order_id: OrderId,
    pub client_id: String,
    pub symbol: Symbol,
    pub status: OrderStatus,
    pub reject_reason: Option<RejectReason>,
    pub filled_qty: u64,
    pub at: Timestamp,
}

impl From<&Order> for OrderUpdate {
    fn from(order: &Order) -> Self {
        Self {
            order_id: order.id,
            client_id: order.client_id.clone(),
            symbol: order.symbol.clone(),
            status: order.status,
            reject_reason: order.reject_reason,
            filled_qty: order.filled_qty,
            at: order.updated_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Fill {
    pub order_id: OrderId,
    pub client_id: String,
    pub symbol: Symbol,
    pub at: Timestamp,
    pub qty: i64,
    pub price: Decimal,
    pub commission: Decimal,
    pub feed: Feed,
}

pub fn target_weight_qty(weight: Decimal, equity: Decimal, price: Decimal) -> Option<i64> {
    weight
        .checked_mul(equity)?
        .checked_div(price)?
        .trunc()
        .to_i64()
}
