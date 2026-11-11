use std::time::Duration;

use buizz_core::market::{Bar, Calendar, CorporateAction, Feed, Instrument, Session, Symbol, Timeframe};
use buizz_core::time::Timestamp;
use chrono::NaiveDate;

use crate::http::{RateLimit, SourceError};

pub const DATA_URL: &str = "https://data.alpaca.markets";
pub const TRADING_URL: &str = "https://paper-api.alpaca.markets";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 200,
    per: Duration::from_secs(60),
};
pub const HISTORY_START: NaiveDate = NaiveDate::from_ymd_opt(2016, 1, 1).expect("valid date");
pub const SIP_DELAY: Duration = Duration::from_secs(15 * 60);
pub const MAX_PAGE_LIMIT: u32 = 10_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlpacaConfig {
    pub key_id: String,
    pub secret_key: String,
    pub data_url: String,
    pub trading_url: String,
}

impl AlpacaConfig {
    pub fn new(key_id: String, secret_key: String) -> Self {
        Self {
            key_id,
            secret_key,
            data_url: DATA_URL.to_owned(),
            trading_url: TRADING_URL.to_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarsRequest {
    pub symbols: Vec<Symbol>,
    pub timeframe: Timeframe,
    pub from: Timestamp,
    pub to: Timestamp,
    pub feed: Feed,
}

#[derive(Clone)]
pub struct AlpacaClient {}

impl AlpacaClient {
    pub fn new(_config: AlpacaConfig) -> Result<Self, SourceError> {
        unimplemented!()
    }

    pub async fn bars(&self, _request: &BarsRequest, _calendar: &Calendar) -> Result<Vec<Bar>, SourceError> {
        unimplemented!()
    }

    pub async fn calendar(&self, _from: NaiveDate, _to: NaiveDate) -> Result<Vec<Session>, SourceError> {
        unimplemented!()
    }

    pub async fn assets(&self) -> Result<Vec<Instrument>, SourceError> {
        unimplemented!()
    }

    pub async fn corporate_actions(
        &self,
        _symbols: &[Symbol],
        _from: NaiveDate,
        _to: NaiveDate,
    ) -> Result<Vec<CorporateAction>, SourceError> {
        unimplemented!()
    }
}
