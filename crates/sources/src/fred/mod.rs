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
const EARLIEST_VINTAGE: NaiveDate = NaiveDate::from_ymd_opt(1776, 7, 4).expect("valid date");
const LATEST_VINTAGE: NaiveDate = NaiveDate::from_ymd_opt(9999, 12, 31).expect("valid date");
const MAX_VINTAGES_PER_REQUEST: usize = 2000;
const PAGE_LIMIT: usize = 100_000;
const VINTAGE_DATE_PAGE_LIMIT: usize = 10_000;
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
struct WireObservation {
    realtime_start: NaiveDate,
    realtime_end: NaiveDate,
    date: NaiveDate,
    value: String,
}

#[derive(Debug, Deserialize)]
struct WireVintageDates {
    count: usize,
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
        let id = series_id(series)?;
        let url = format!("{}/fred/series/vintagedates", self.config.base_url);
        let since = since.to_string();
        let limit = VINTAGE_DATE_PAGE_LIMIT.to_string();
        let mut dates: Vec<NaiveDate> = Vec::new();
        loop {
            let offset = dates.len().to_string();
            let request = self.http.client().get(&url).query(&[
                ("series_id", id),
                ("api_key", self.config.api_key.as_str()),
                ("file_type", "json"),
                ("realtime_start", since.as_str()),
                ("limit", &limit),
                ("offset", &offset),
            ]);
            let body = self.http.fetch(request).await.map_err(without_url)?;
            let page: WireVintageDates = decode(&body)?;
            let received = page.vintage_dates.len();
            dates.extend(page.vintage_dates);
            if received == 0 || dates.len() >= page.count {
                return Ok(dates);
            }
        }
    }

    pub async fn vintages(
        &self,
        series: MacroSeries,
    ) -> Result<Vec<MacroObservation>, SourceError> {
        let dates = self.vintage_dates_since(series, EARLIEST_VINTAGE).await?;
        let starts: Vec<NaiveDate> = dates
            .chunks(MAX_VINTAGES_PER_REQUEST)
            .map(|chunk| chunk[0])
            .collect();
        let mut observations: Vec<WireObservation> = Vec::new();
        for (index, start) in starts.iter().enumerate() {
            let end = starts
                .get(index + 1)
                .and_then(|next| next.pred_opt())
                .unwrap_or(LATEST_VINTAGE);
            observations.extend(self.observations(series, *start, end).await?);
        }
        normalize(series, stitch(observations))
    }

    async fn observations(
        &self,
        series: MacroSeries,
        realtime_start: NaiveDate,
        realtime_end: NaiveDate,
    ) -> Result<Vec<WireObservation>, SourceError> {
        let id = series_id(series)?;
        let url = format!("{}/fred/series/observations", self.config.base_url);
        let realtime_start = realtime_start.to_string();
        let realtime_end = realtime_end.to_string();
        let limit = PAGE_LIMIT.to_string();
        let mut observations: Vec<WireObservation> = Vec::new();
        loop {
            let offset = observations.len().to_string();
            let request = self.http.client().get(&url).query(&[
                ("series_id", id),
                ("api_key", self.config.api_key.as_str()),
                ("file_type", "json"),
                ("realtime_start", realtime_start.as_str()),
                ("realtime_end", realtime_end.as_str()),
                ("limit", &limit),
                ("offset", &offset),
            ]);
            let body = self.http.fetch(request).await.map_err(without_url)?;
            let page: WirePage = decode(&body)?;
            let received = page.observations.len();
            observations.extend(page.observations);
            if received == 0 || observations.len() >= page.count {
                return Ok(observations);
            }
        }
    }
}

fn series_id(series: MacroSeries) -> Result<&'static str, SourceError> {
    series
        .fred_id()
        .ok_or_else(|| SourceError::Decode(format!("{series:?} is not a FRED series")))
}

fn stitch(mut observations: Vec<WireObservation>) -> Vec<WireObservation> {
    observations.sort_by_key(|observation| (observation.date, observation.realtime_start));
    let mut stitched: Vec<WireObservation> = Vec::with_capacity(observations.len());
    for observation in observations {
        match stitched.last_mut() {
            Some(previous)
                if previous.date == observation.date
                    && previous.value == observation.value
                    && previous.realtime_end.succ_opt() == Some(observation.realtime_start) =>
            {
                previous.realtime_end = observation.realtime_end;
            }
            _ => stitched.push(observation),
        }
    }
    stitched
}

fn normalize(
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

fn decode<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, SourceError> {
    serde_json::from_slice(body).map_err(|error| SourceError::Decode(error.to_string()))
}

fn without_url(error: SourceError) -> SourceError {
    match error {
        SourceError::Http(error) => SourceError::Http(error.without_url()),
        other => other,
    }
}
