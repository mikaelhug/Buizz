use std::collections::BTreeSet;
use std::time::Duration;

use buizz_core::market::{Symbol, TradingHaltRecord};
use buizz_core::time::Timestamp;
use chrono::{NaiveDate, NaiveTime};
use roxmltree::{Document, Node};
use rust_decimal::Decimal;

use crate::http::{HttpClient, RateLimit, SourceError};

pub const BASE_URL: &str = "https://www.nasdaqtrader.com";
pub const URL_ENV: &str = "NASDAQ_TRADER_URL";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 1,
    per: Duration::from_secs(1),
    cooldown: Duration::from_secs(60),
};
const SYMBOL_DIRECTORY: &str = "/dynamic/SymDir/nasdaqtraded.txt";
pub const HALTS_PATH: &str = "/rss.aspx";
const HALTS_FEED: &str = "tradehalts";
const HALT_NAMESPACE: &str = "http://www.nasdaqtrader.com/";
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

    pub async fn trading_halts(&self) -> Result<Vec<TradingHaltRecord>, SourceError> {
        let url = format!("{}{HALTS_PATH}", self.base_url);
        let request = self.http.client().get(url).query(&[("feed", HALTS_FEED)]);
        let body = self.http.fetch(request).await?;
        normalize_trading_halts(&String::from_utf8_lossy(&body))
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

fn halt_error(message: impl std::fmt::Display) -> SourceError {
    SourceError::Decode(format!("Nasdaq trade halts feed: {message}"))
}

pub fn normalize_trading_halts(text: &str) -> Result<Vec<TradingHaltRecord>, SourceError> {
    let document = Document::parse(text).map_err(halt_error)?;
    if !document
        .descendants()
        .any(|node| node.has_tag_name("channel"))
    {
        return Err(halt_error("no channel"));
    }
    document
        .descendants()
        .filter(|node| node.has_tag_name("item"))
        .map(halt)
        .collect()
}

fn field<'a>(item: Node<'a, '_>, name: &str) -> Option<&'a str> {
    item.children()
        .find(|node| {
            node.tag_name().name() == name
                && node
                    .tag_name()
                    .namespace()
                    .is_none_or(|namespace| namespace.starts_with(HALT_NAMESPACE))
        })
        .and_then(|node| node.text())
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn halt_date(text: &str) -> Result<NaiveDate, SourceError> {
    NaiveDate::parse_from_str(text, "%m/%d/%Y").map_err(|_| halt_error(format!("date {text:?}")))
}

fn halt_time(date: NaiveDate, text: &str) -> Result<Timestamp, SourceError> {
    NaiveTime::parse_from_str(text, "%H:%M:%S%.f")
        .ok()
        .and_then(|time| Timestamp::at_exchange_time(date, time))
        .ok_or_else(|| halt_error(format!("time {text:?}")))
}

fn halt(item: Node<'_, '_>) -> Result<TradingHaltRecord, SourceError> {
    let required = |name: &str| field(item, name).ok_or_else(|| halt_error(format!("no {name}")));
    let date = halt_date(required("HaltDate")?)?;
    let resumption_date = field(item, "ResumptionDate").map(halt_date).transpose()?;
    let resumes_at = |name: &str| {
        field(item, name)
            .zip(resumption_date)
            .map(|(time, date)| halt_time(date, time))
            .transpose()
    };
    Ok(TradingHaltRecord {
        symbol: Symbol::new(required("IssueSymbol")?),
        halted_at: halt_time(date, required("HaltTime")?)?,
        name: field(item, "IssueName").unwrap_or_default().to_owned(),
        market: field(item, "Market").unwrap_or_default().to_owned(),
        reason: required("ReasonCode")?.to_owned(),
        pause_threshold: field(item, "PauseThresholdPrice")
            .map(|price| {
                price
                    .parse::<Decimal>()
                    .map_err(|_| halt_error(format!("pause threshold {price:?}")))
            })
            .transpose()?,
        resumption_date,
        quotes_resume_at: resumes_at("ResumptionQuoteTime")?,
        trading_resumes_at: resumes_at("ResumptionTradeTime")?,
    })
}
