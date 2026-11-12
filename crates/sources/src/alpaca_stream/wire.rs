use buizz_core::market::{Bar, Feed, Symbol};
use buizz_core::time::Timestamp;
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::alpaca::WireNews;
use crate::http::decimal;

pub const AUTH_FAILED_CODES: [u16; 4] = [401, 402, 403, 404];
pub const SYMBOL_LIMIT_CODE: u16 = 405;

#[derive(Debug, Deserialize)]
#[serde(tag = "T")]
pub enum Frame {
    #[serde(rename = "success")]
    Success { msg: String },
    #[serde(rename = "error")]
    Error { code: u16, msg: String },
    #[serde(rename = "subscription")]
    Subscription {
        #[serde(default)]
        bars: Vec<Symbol>,
    },
    #[serde(rename = "b")]
    Bar(WireBar),
    #[serde(rename = "n")]
    News(WireNews),
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub struct WireBar {
    #[serde(rename = "S")]
    symbol: Symbol,
    #[serde(rename = "o", deserialize_with = "decimal")]
    open: Decimal,
    #[serde(rename = "h", deserialize_with = "decimal")]
    high: Decimal,
    #[serde(rename = "l", deserialize_with = "decimal")]
    low: Decimal,
    #[serde(rename = "c", deserialize_with = "decimal")]
    close: Decimal,
    #[serde(rename = "v")]
    volume: u64,
    #[serde(rename = "t")]
    start: Timestamp,
}

impl From<WireBar> for Bar {
    fn from(wire: WireBar) -> Self {
        Self {
            symbol: wire.symbol,
            start: wire.start,
            open: wire.open,
            high: wire.high,
            low: wire.low,
            close: wire.close,
            volume: wire.volume,
            feed: Feed::Iex,
        }
    }
}
