use std::collections::BTreeMap;
use std::time::Duration;

use buizz_core::market::{Calendar, FundamentalFact, FundamentalMetric, Symbol};

use crate::http::{RateLimit, SourceError};

pub const DATA_URL: &str = "https://data.sec.gov";
pub const WWW_URL: &str = "https://www.sec.gov";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 10,
    per: Duration::from_secs(1),
};
pub const ACCEPTED_FORMS: [&str; 8] = [
    "10-K", "10-K/A", "10-Q", "10-Q/A", "20-F", "20-F/A", "40-F", "40-F/A",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgarConfig {
    pub user_agent: String,
    pub data_url: String,
    pub www_url: String,
}

impl EdgarConfig {
    pub fn new(user_agent: String) -> Self {
        Self {
            user_agent,
            data_url: DATA_URL.to_owned(),
            www_url: WWW_URL.to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitKind {
    Currency,
    Shares,
    CurrencyPerShare,
}

#[derive(Clone)]
pub struct EdgarClient {}

impl EdgarClient {
    pub fn new(_config: EdgarConfig) -> Result<Self, SourceError> {
        unimplemented!()
    }

    pub async fn ticker_map(&self) -> Result<BTreeMap<Symbol, u32>, SourceError> {
        unimplemented!()
    }

    pub async fn company_facts(
        &self,
        _symbol: &Symbol,
        _cik: u32,
        _calendar: &Calendar,
    ) -> Result<Vec<FundamentalFact>, SourceError> {
        unimplemented!()
    }
}

pub fn normalize_company_facts(
    _symbol: &Symbol,
    _payload: &[u8],
    _calendar: &Calendar,
) -> Result<Vec<FundamentalFact>, SourceError> {
    unimplemented!()
}

pub fn metric_for(_taxonomy: &str, _concept: &str) -> Option<(FundamentalMetric, UnitKind)> {
    unimplemented!()
}
