use std::collections::BTreeSet;
use std::time::Duration;

use buizz_core::market::Symbol;

use crate::http::{HttpClient, RateLimit, SourceError};

pub const BASE_URL: &str = "https://www.nasdaqtrader.com";
pub const URL_ENV: &str = "NASDAQ_TRADER_URL";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 1,
    per: Duration::from_secs(1),
    cooldown: Duration::from_secs(60),
};
const SYMBOL_DIRECTORY: &str = "/dynamic/SymDir/nasdaqtraded.txt";
const HEADER: &str = "Nasdaq Traded|Symbol|Security Name|Listing Exchange|Market Category|ETF|";
const TRAILER: &str = "File Creation Time:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NasdaqConfig {
    pub base_url: String,
}

#[derive(Clone)]
pub struct NasdaqClient {
    http: HttpClient,
    base_url: String,
}

impl NasdaqClient {
    pub fn new(config: NasdaqConfig) -> Result<Self, SourceError> {
        Ok(Self {
            http: HttpClient::new("buizz", RATE_LIMIT)?,
            base_url: config.base_url.trim_end_matches('/').to_owned(),
        })
    }

    pub async fn etf_symbols(&self) -> Result<BTreeSet<Symbol>, SourceError> {
        let url = format!("{}{SYMBOL_DIRECTORY}", self.base_url);
        let body = self.http.fetch(self.http.client().get(url)).await?;
        normalize_etf_symbols(&String::from_utf8_lossy(&body))
    }
}

pub fn normalize_etf_symbols(text: &str) -> Result<BTreeSet<Symbol>, SourceError> {
    let mut lines = text.lines();
    if !lines
        .next()
        .is_some_and(|header| header.starts_with(HEADER))
    {
        return Err(SourceError::Decode(
            "NASDAQ symbol directory: unexpected header".to_owned(),
        ));
    }
    let mut etfs = BTreeSet::new();
    for line in lines {
        if line.starts_with(TRAILER) {
            return Ok(etfs);
        }
        let fields: Vec<&str> = line.split('|').collect();
        if let [_, symbol, _, _, _, "Y", _, "N", ..] = fields.as_slice() {
            etfs.insert(Symbol::new(symbol));
        }
    }
    Err(SourceError::Decode(
        "NASDAQ symbol directory: no trailer, the download is incomplete".to_owned(),
    ))
}
