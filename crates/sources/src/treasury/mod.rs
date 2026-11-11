use std::time::Duration;

use buizz_core::market::RiskFreeRate;

use crate::http::{RateLimit, SourceError};

pub const BASE_URL: &str = "https://home.treasury.gov";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 1,
    per: Duration::from_secs(1),
};
pub const FIRST_YEAR: i32 = 1990;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreasuryConfig {
    pub base_url: String,
}

impl Default for TreasuryConfig {
    fn default() -> Self {
        Self {
            base_url: BASE_URL.to_owned(),
        }
    }
}

#[derive(Clone)]
pub struct TreasuryClient {}

impl TreasuryClient {
    pub fn new(_config: TreasuryConfig) -> Result<Self, SourceError> {
        unimplemented!()
    }

    pub async fn three_month_yields(&self, _year: i32) -> Result<Vec<RiskFreeRate>, SourceError> {
        unimplemented!()
    }
}

pub fn parse_par_yield_csv(_csv: &str) -> Result<Vec<RiskFreeRate>, SourceError> {
    unimplemented!()
}
