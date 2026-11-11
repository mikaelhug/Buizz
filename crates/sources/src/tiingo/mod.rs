use std::time::Duration;

use buizz_core::market::{Bar, Calendar, CorporateAction, Symbol};
use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::http::{RateLimit, SourceError};

pub const BASE_URL: &str = "https://api.tiingo.com";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 50,
    per: Duration::from_secs(60 * 60),
};
pub const REQUESTS_PER_DAY: u32 = 1_000;
pub const SYMBOLS_PER_MONTH: u32 = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TiingoConfig {
    pub api_key: String,
    pub base_url: String,
}

impl TiingoConfig {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: BASE_URL.to_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TiingoDaily {
    pub date: NaiveDate,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: u64,
    pub div_cash: Decimal,
    pub split_factor: Decimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverlapVerdict {
    Trusted { overlap: usize },
    Insufficient { overlap: usize },
    Rejected { reason: String },
}

#[derive(Clone)]
pub struct TiingoClient {}

impl TiingoClient {
    pub fn new(_config: TiingoConfig) -> Result<Self, SourceError> {
        unimplemented!()
    }

    pub async fn daily(
        &self,
        _symbol: &Symbol,
        _from: NaiveDate,
        _to: NaiveDate,
    ) -> Result<Vec<TiingoDaily>, SourceError> {
        unimplemented!()
    }
}

pub fn normalize(
    _symbol: &Symbol,
    _rows: &[TiingoDaily],
    _calendar: &Calendar,
) -> (Vec<Bar>, Vec<CorporateAction>) {
    unimplemented!()
}

pub fn validate_overlap(_primary: &[Bar], _secondary: &[Bar]) -> OverlapVerdict {
    unimplemented!()
}
