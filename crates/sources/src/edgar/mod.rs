use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use buizz_core::market::{Calendar, FundamentalFact, Symbol};
use serde::Deserialize;

use crate::http::{HttpClient, RateLimit, SourceError};

mod concepts;
mod normalize;

pub use normalize::normalize_company_facts;

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
pub struct EdgarClient {
    http: HttpClient,
    data_url: String,
    www_url: String,
}

#[derive(Deserialize)]
struct TickerEntry {
    cik_str: u32,
    ticker: String,
}

#[derive(Deserialize)]
struct FundTable {
    fields: Vec<String>,
    data: Vec<Vec<serde_json::Value>>,
}

fn symbol(ticker: &str) -> Symbol {
    Symbol::new(&ticker.replace('-', "."))
}

impl EdgarClient {
    pub fn new(config: EdgarConfig) -> Result<Self, SourceError> {
        Ok(Self {
            http: HttpClient::new(&config.user_agent, RATE_LIMIT)?,
            data_url: config.data_url.trim_end_matches('/').to_owned(),
            www_url: config.www_url.trim_end_matches('/').to_owned(),
        })
    }

    pub async fn ticker_map(&self) -> Result<BTreeMap<Symbol, u32>, SourceError> {
        let url = format!("{}/files/company_tickers.json", self.www_url);
        let body = self.fetch(url).await?;
        let entries: BTreeMap<String, TickerEntry> = serde_json::from_slice(&body)
            .map_err(|error| SourceError::Decode(error.to_string()))?;
        Ok(entries
            .into_values()
            .map(|entry| (symbol(&entry.ticker), entry.cik_str))
            .collect())
    }

    pub async fn fund_tickers(&self) -> Result<BTreeSet<Symbol>, SourceError> {
        let url = format!("{}/files/company_tickers_mf.json", self.www_url);
        let body = self.fetch(url).await?;
        let table: FundTable = serde_json::from_slice(&body)
            .map_err(|error| SourceError::Decode(error.to_string()))?;
        let column = table
            .fields
            .iter()
            .position(|field| field == "symbol")
            .ok_or_else(|| SourceError::Decode("fund tickers without a symbol field".to_owned()))?;
        Ok(table
            .data
            .iter()
            .filter_map(|row| row.get(column)?.as_str().map(symbol))
            .collect())
    }

    pub async fn company_facts(
        &self,
        symbol: &Symbol,
        cik: u32,
        calendar: &Calendar,
    ) -> Result<Vec<FundamentalFact>, SourceError> {
        let url = format!("{}/api/xbrl/companyfacts/CIK{cik:010}.json", self.data_url);
        let body = self.fetch(url).await?;
        normalize_company_facts(symbol, &body, calendar)
    }

    async fn fetch(&self, url: String) -> Result<Vec<u8>, SourceError> {
        let response = self.http.send(self.http.client().get(url)).await?;
        Ok(response.bytes().await?.to_vec())
    }
}
