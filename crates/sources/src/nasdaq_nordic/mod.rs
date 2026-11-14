use std::collections::BTreeSet;
use std::time::Duration;

use buizz_core::market::{NordicBars, NordicListing, Symbol};
use chrono::NaiveDate;
use reqwest::header::ACCEPT;
use serde::Deserialize;

use crate::http::{HttpClient, RateLimit, SourceError, decode_json};
use crate::nordic::{BROWSER_USER_AGENT, DayRow, daily_bars};

pub const BASE_URL: &str = "https://api.nasdaq.com";
pub const URL_ENV: &str = "NASDAQ_NORDIC_URL";
pub const SHARES_PATH: &str = "/api/nordic/screener/shares";
pub const INSTRUMENTS_PATH: &str = "/api/nordic/instruments";
pub const HISTORY_MONTHS: u32 = 120;
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 1,
    per: Duration::from_secs(2),
    cooldown: Duration::from_secs(300),
};
pub const LISTS: [(&str, &str, &str); 8] = [
    ("STO", "MAIN_MARKET", "XSTO"),
    ("HEL", "MAIN_MARKET", "XHEL"),
    ("CPH", "MAIN_MARKET", "XCSE"),
    ("ICE", "MAIN_MARKET", "XICE"),
    ("STO", "FIRST_NORTH", "SSME"),
    ("HEL", "FIRST_NORTH", "FNFI"),
    ("CPH", "FIRST_NORTH", "FNDK"),
    ("ICE", "FIRST_NORTH", "FNIS"),
];
const OK: u16 = 200;
const DATE_FORMAT: &str = "%Y-%m-%d";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NasdaqNordicConfig {
    pub base_url: String,
}

#[derive(Clone)]
pub struct NasdaqNordicClient {
    http: HttpClient,
    base_url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<T> {
    data: Option<T>,
    status: Status,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    r_code: u16,
    b_code_message: Option<Vec<CodeMessage>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodeMessage {
    error_message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Screener {
    instrument_listing: Option<InstrumentListing>,
}

#[derive(Deserialize)]
struct InstrumentListing {
    rows: Option<Vec<ListedShare>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListedShare {
    orderbook_id: String,
    symbol: String,
    full_name: String,
    isin: String,
    currency: String,
    #[serde(default)]
    sector: String,
}

#[derive(Deserialize)]
struct Chart {
    #[serde(rename = "CP")]
    points: Option<Vec<Point>>,
}

#[derive(Deserialize)]
struct Point {
    z: Day,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Day {
    date_time: String,
    #[serde(default)]
    open: String,
    #[serde(default)]
    high: String,
    #[serde(default)]
    low: String,
    #[serde(default)]
    close: String,
    #[serde(default)]
    volume: String,
}

impl NasdaqNordicClient {
    pub fn new(config: NasdaqNordicConfig) -> Result<Self, SourceError> {
        Ok(Self {
            http: HttpClient::new(BROWSER_USER_AGENT, RATE_LIMIT)?,
            base_url: config.base_url.trim_end_matches('/').to_owned(),
        })
    }

    pub async fn listings(&self) -> Result<Vec<NordicListing>, SourceError> {
        let mut seen = BTreeSet::new();
        let mut listings = Vec::new();
        for (market, category, mic) in LISTS {
            let request = self
                .http
                .client()
                .get(format!("{}{SHARES_PATH}", self.base_url))
                .header(ACCEPT, "application/json")
                .query(&[
                    ("category", category),
                    ("tableonly", "false"),
                    ("market", market),
                ]);
            let body = self.http.fetch(request).await?;
            listings.extend(
                normalize_listings(&body, mic)?
                    .into_iter()
                    .filter(|listing| seen.insert(listing.listing.clone())),
            );
        }
        Ok(listings)
    }

    pub async fn daily_bars(
        &self,
        listing: &Symbol,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<NordicBars, SourceError> {
        let request = self
            .http
            .client()
            .get(format!(
                "{}{INSTRUMENTS_PATH}/{listing}/chart",
                self.base_url
            ))
            .header(ACCEPT, "application/json")
            .query(&[
                ("assetClass", "SHARES".to_owned()),
                ("fromDate", from.format(DATE_FORMAT).to_string()),
                ("toDate", to.format(DATE_FORMAT).to_string()),
            ]);
        normalize_chart(&self.http.fetch(request).await?)
    }
}

fn unwrap<T>(envelope: Envelope<T>) -> Result<Option<T>, SourceError> {
    if envelope.status.r_code == OK {
        return Ok(envelope.data);
    }
    let body = envelope
        .status
        .b_code_message
        .unwrap_or_default()
        .into_iter()
        .map(|message| message.error_message)
        .collect::<Vec<_>>()
        .join("; ");
    Err(SourceError::Status {
        status: envelope.status.r_code,
        body,
    })
}

pub fn normalize_listings(body: &[u8], mic: &str) -> Result<Vec<NordicListing>, SourceError> {
    let screener = unwrap(decode_json::<Envelope<Screener>>(body)?)?
        .ok_or_else(|| SourceError::Decode(format!("Nasdaq Nordic {mic} share list: no data")))?;
    Ok(screener
        .instrument_listing
        .and_then(|listing| listing.rows)
        .unwrap_or_default()
        .into_iter()
        .filter(|share| !share.orderbook_id.trim().is_empty())
        .map(|share| NordicListing {
            listing: Symbol::new(&share.orderbook_id),
            mic: mic.to_owned(),
            isin: share.isin.trim().to_owned(),
            symbol: share.symbol.trim().to_owned(),
            name: share.full_name.trim().to_owned(),
            currency: share.currency.trim().to_owned(),
            sector: Some(share.sector.trim().to_owned()).filter(|sector| !sector.is_empty()),
        })
        .collect())
}

pub fn normalize_chart(body: &[u8]) -> Result<NordicBars, SourceError> {
    let points = unwrap(decode_json::<Envelope<Chart>>(body)?)?
        .and_then(|chart| chart.points)
        .unwrap_or_default();
    let mut undated = 0;
    let mut daily = daily_bars(points.iter().filter_map(|point| {
        let day = &point.z;
        let Ok(date) = NaiveDate::parse_from_str(day.date_time.trim(), DATE_FORMAT) else {
            undated += 1;
            return None;
        };
        Some(DayRow {
            date,
            open: &day.open,
            high: &day.high,
            low: &day.low,
            close: &day.close,
            volume: &day.volume,
        })
    }));
    daily.skipped += undated;
    Ok(daily)
}
