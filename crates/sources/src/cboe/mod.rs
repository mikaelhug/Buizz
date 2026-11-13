use std::time::Duration;

use buizz_core::market::{MacroObservation, MacroSeries};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::http::{HttpClient, RateLimit, SourceError};

pub const BASE_URL: &str = "https://cdn.cboe.com";
pub const URL_ENV: &str = "CBOE_URL";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 1,
    per: Duration::from_secs(1),
    cooldown: Duration::from_secs(60),
};
pub const LAST_ARCHIVED_PUT_CALL: NaiveDate =
    NaiveDate::from_ymd_opt(2019, 10, 4).expect("valid date");
pub const FIRST_DAILY_PUT_CALL: NaiveDate =
    NaiveDate::from_ymd_opt(2019, 10, 7).expect("valid date");
const DATE_COLUMN: &str = "DATE";
const CLOSE_COLUMN: &str = "CLOSE";
const RATIO_COLUMN: &str = "P/C Ratio";
const NOT_PUBLISHED: [u16; 2] = [403, 404];

const fn day(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("valid date")
}

pub fn first_published(series: MacroSeries) -> Option<NaiveDate> {
    match series {
        MacroSeries::Vix => Some(day(2003, 9, 22)),
        MacroSeries::Skew => Some(day(2011, 2, 23)),
        MacroSeries::Vvix => Some(day(2012, 3, 14)),
        MacroSeries::Vix9d => Some(day(2013, 10, 1)),
        MacroSeries::Cor3m => Some(day(2021, 7, 1)),
        MacroSeries::Cor1m => Some(day(2022, 7, 18)),
        _ => None,
    }
}

fn index_name(series: MacroSeries) -> Option<&'static str> {
    match series {
        MacroSeries::Vix => Some("VIX"),
        MacroSeries::Vix3m => Some("VIX3M"),
        MacroSeries::Vix9d => Some("VIX9D"),
        MacroSeries::Vvix => Some("VVIX"),
        MacroSeries::Skew => Some("SKEW"),
        MacroSeries::Cor1m => Some("COR1M"),
        MacroSeries::Cor3m => Some("COR3M"),
        _ => None,
    }
}

fn put_call_product(series: MacroSeries) -> Option<(&'static str, &'static str)> {
    match series {
        MacroSeries::PutCallTotal => Some(("totalpc", "TOTAL PUT/CALL RATIO")),
        MacroSeries::PutCallEquity => Some(("equitypc", "EQUITY PUT/CALL RATIO")),
        MacroSeries::PutCallIndex => Some(("indexpc", "INDEX PUT/CALL RATIO")),
        _ => None,
    }
}

#[derive(Deserialize)]
struct DailyOptions {
    ratios: Vec<Ratio>,
}

#[derive(Deserialize)]
struct Ratio {
    name: String,
    value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CboeConfig {
    pub base_url: String,
}

#[derive(Clone)]
pub struct CboeClient {
    http: HttpClient,
    base_url: String,
}

impl CboeClient {
    pub fn new(config: CboeConfig) -> Result<Self, SourceError> {
        Ok(Self {
            http: HttpClient::new("buizz", RATE_LIMIT)?,
            base_url: config.base_url.trim_end_matches('/').to_owned(),
        })
    }

    pub async fn daily_closes(
        &self,
        series: MacroSeries,
    ) -> Result<Vec<MacroObservation>, SourceError> {
        let index = index_name(series)
            .ok_or_else(|| SourceError::Decode(format!("{series:?} is not a CBOE index")))?;
        let url = format!(
            "{}/api/global/us_indices/daily_prices/{index}_History.csv",
            self.base_url
        );
        let csv = self.http.fetch(self.http.client().get(url)).await?;
        parse_history_csv(series, &String::from_utf8_lossy(&csv))
    }

    pub async fn archived_put_call(
        &self,
        series: MacroSeries,
    ) -> Result<Vec<MacroObservation>, SourceError> {
        let (file, _) = put_call_product(series)
            .ok_or_else(|| SourceError::Decode(format!("{series:?} is not a put/call ratio")))?;
        let url = format!(
            "{}/resources/options/volume_and_call_put_ratios/{file}.csv",
            self.base_url
        );
        let csv = self.http.fetch(self.http.client().get(url)).await?;
        parse_put_call_archive(series, &String::from_utf8_lossy(&csv))
    }

    pub async fn daily_put_call(
        &self,
        date: NaiveDate,
    ) -> Result<Option<Vec<MacroObservation>>, SourceError> {
        let url = format!(
            "{}/data/us/options/market_statistics/daily/{date}_daily_options",
            self.base_url
        );
        match self.http.fetch(self.http.client().get(url)).await {
            Ok(body) => parse_daily_put_call(date, &body).map(Some),
            Err(SourceError::Status { status, .. }) if NOT_PUBLISHED.contains(&status) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

pub fn parse_put_call_archive(
    series: MacroSeries,
    csv: &str,
) -> Result<Vec<MacroObservation>, SourceError> {
    let table = csv
        .find("\nDATE,")
        .map(|start| &csv[start + 1..])
        .ok_or_else(|| SourceError::Decode(format!("{series:?} archive without a DATE header")))?;
    let mut reader = csv::Reader::from_reader(table.as_bytes());
    let headers = reader.headers().map_err(decode)?.clone();
    let ratio_column = headers
        .iter()
        .position(|header| header.trim() == RATIO_COLUMN)
        .ok_or_else(|| SourceError::Decode(format!("missing {RATIO_COLUMN:?} column")))?;
    let mut ratios = Vec::new();
    for record in reader.records() {
        let record = record.map_err(decode)?;
        let date = parse_date(record.get(0).unwrap_or_default())?;
        ratios.push(MacroObservation {
            series,
            date,
            published: date,
            value: Some(parse_value(record.get(ratio_column).unwrap_or_default())?),
        });
    }
    ratios.sort_by_key(|ratio| ratio.date);
    Ok(ratios)
}

pub fn parse_daily_put_call(
    date: NaiveDate,
    body: &[u8],
) -> Result<Vec<MacroObservation>, SourceError> {
    let daily: DailyOptions = serde_json::from_slice(body)
        .map_err(|error| SourceError::Decode(format!("daily options {date}: {error}")))?;
    MacroSeries::PUT_CALL
        .into_iter()
        .map(|series| {
            let (_, name) = put_call_product(series).expect("a put/call series");
            let ratio = daily
                .ratios
                .iter()
                .find(|ratio| ratio.name.trim() == name)
                .ok_or_else(|| SourceError::Decode(format!("{date} has no {name:?}")))?;
            Ok(MacroObservation {
                series,
                date,
                published: date,
                value: Some(parse_value(&ratio.value)?),
            })
        })
        .collect()
}

fn parse_date(text: &str) -> Result<NaiveDate, SourceError> {
    let text = text.trim();
    NaiveDate::parse_from_str(text, "%m/%d/%Y")
        .map_err(|error| SourceError::Decode(format!("date {text:?}: {error}")))
}

fn parse_value(text: &str) -> Result<Decimal, SourceError> {
    let text = text.trim();
    text.parse::<Decimal>()
        .map_err(|error| SourceError::Decode(format!("value {text:?}: {error}")))
}

pub fn parse_history_csv(
    series: MacroSeries,
    csv: &str,
) -> Result<Vec<MacroObservation>, SourceError> {
    let mut reader = csv::Reader::from_reader(csv.as_bytes());
    let headers = reader.headers().map_err(decode)?.clone();
    let column = |name: &str| {
        headers
            .iter()
            .position(|header| header.trim() == name)
            .ok_or_else(|| SourceError::Decode(format!("missing {name:?} column")))
    };
    let date_column = column(DATE_COLUMN)?;
    let close_column =
        column(CLOSE_COLUMN).or_else(|error| index_name(series).map_or(Err(error), column))?;
    let launched = first_published(series);
    let mut closes = Vec::new();
    for record in reader.records() {
        let record = record.map_err(decode)?;
        let date = parse_date(record.get(date_column).unwrap_or_default())?;
        if launched.is_some_and(|launch| date < launch) {
            continue;
        }
        closes.push(MacroObservation {
            series,
            date,
            published: date,
            value: Some(parse_value(record.get(close_column).unwrap_or_default())?),
        });
    }
    closes.sort_by_key(|close| close.date);
    Ok(closes)
}

fn decode(error: csv::Error) -> SourceError {
    SourceError::Decode(error.to_string())
}
