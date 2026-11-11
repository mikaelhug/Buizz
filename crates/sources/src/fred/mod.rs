use std::fmt;
use std::time::Duration;

use buizz_core::market::{MacroObservation, MacroSeries};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::http::{HttpClient, REDACTED, RateLimit, SourceError};

pub const BASE_URL: &str = "https://api.stlouisfed.org";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 80,
    per: Duration::from_secs(60),
    cooldown: Duration::from_secs(60),
};
pub const KEY_ENV: &str = "FRED_API_KEY";
pub const URL_ENV: &str = "FRED_URL";
const EVERY_VINTAGE: [(&str, &str); 2] = [
    ("realtime_start", "1776-07-04"),
    ("realtime_end", "9999-12-31"),
];
const PAGE_LIMIT: usize = 100_000;
const MISSING: &str = ".";

#[derive(Clone, PartialEq, Eq)]
pub struct FredConfig {
    pub api_key: String,
    pub base_url: String,
}

impl fmt::Debug for FredConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FredConfig")
            .field("api_key", &REDACTED)
            .field("base_url", &self.base_url)
            .finish()
    }
}

impl FredConfig {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: BASE_URL.to_owned(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct WireObservation {
    pub realtime_start: NaiveDate,
    pub date: NaiveDate,
    pub value: String,
}

#[derive(Debug, Deserialize)]
struct WireVintageDates {
    vintage_dates: Vec<NaiveDate>,
}

#[derive(Debug, Deserialize)]
struct WirePage {
    count: usize,
    observations: Vec<WireObservation>,
}

#[derive(Clone)]
pub struct FredClient {
    http: HttpClient,
    config: FredConfig,
}

impl FredClient {
    pub fn new(config: FredConfig) -> Result<Self, SourceError> {
        Ok(Self {
            http: HttpClient::new("buizz", RATE_LIMIT)?,
            config: FredConfig {
                base_url: config.base_url.trim_end_matches('/').to_owned(),
                ..config
            },
        })
    }

    pub async fn vintage_dates_since(
        &self,
        series: MacroSeries,
        since: NaiveDate,
    ) -> Result<Vec<NaiveDate>, SourceError> {
        let id = series
            .fred_id()
            .ok_or_else(|| SourceError::Decode(format!("{series:?} is not a FRED series")))?;
        let url = format!("{}/fred/series/vintagedates", self.config.base_url);
        let since = since.to_string();
        let request = self.http.client().get(&url).query(&[
            ("series_id", id),
            ("api_key", self.config.api_key.as_str()),
            ("file_type", "json"),
            ("realtime_start", since.as_str()),
        ]);
        let page: WireVintageDates = self
            .http
            .send(request)
            .await
            .map_err(without_url)?
            .json()
            .await
            .map_err(|error| SourceError::Http(error.without_url()))?;
        Ok(page.vintage_dates)
    }

    pub async fn vintages(
        &self,
        series: MacroSeries,
    ) -> Result<Vec<MacroObservation>, SourceError> {
        let id = series
            .fred_id()
            .ok_or_else(|| SourceError::Decode(format!("{series:?} is not a FRED series")))?;
        let url = format!("{}/fred/series/observations", self.config.base_url);
        let mut observations: Vec<WireObservation> = Vec::new();
        loop {
            let offset = observations.len().to_string();
            let limit = PAGE_LIMIT.to_string();
            let request = self
                .http
                .client()
                .get(&url)
                .query(&[
                    ("series_id", id),
                    ("api_key", self.config.api_key.as_str()),
                    ("file_type", "json"),
                    ("limit", &limit),
                    ("offset", &offset),
                ])
                .query(&EVERY_VINTAGE);
            let page: WirePage = self
                .http
                .send(request)
                .await
                .map_err(without_url)?
                .json()
                .await
                .map_err(|error| SourceError::Http(error.without_url()))?;
            let received = page.observations.len();
            observations.extend(page.observations);
            if received == 0 || observations.len() >= page.count {
                break;
            }
        }
        normalize(series, observations)
    }
}

pub fn normalize(
    series: MacroSeries,
    observations: Vec<WireObservation>,
) -> Result<Vec<MacroObservation>, SourceError> {
    observations
        .into_iter()
        .map(|observation| {
            let value = match observation.value.trim() {
                MISSING => None,
                text => Some(text.parse::<Decimal>().map_err(|error| {
                    SourceError::Decode(format!("{series:?} value {text:?}: {error}"))
                })?),
            };
            Ok(MacroObservation {
                series,
                date: observation.date,
                published: observation.realtime_start,
                value,
            })
        })
        .collect()
}

fn without_url(error: SourceError) -> SourceError {
    match error {
        SourceError::Http(error) => SourceError::Http(error.without_url()),
        other => other,
    }
}
