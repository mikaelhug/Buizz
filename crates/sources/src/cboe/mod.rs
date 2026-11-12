use std::time::Duration;

use buizz_core::market::{MacroObservation, MacroSeries};
use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::http::{HttpClient, RateLimit, SourceError};

pub const BASE_URL: &str = "https://cdn.cboe.com";
pub const URL_ENV: &str = "CBOE_URL";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 1,
    per: Duration::from_secs(1),
    cooldown: Duration::from_secs(60),
};
const VIX_METHODOLOGY_START: NaiveDate = NaiveDate::from_ymd_opt(2003, 9, 22).expect("valid date");
const DATE_COLUMN: &str = "DATE";
const CLOSE_COLUMN: &str = "CLOSE";

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
        let index = match series {
            MacroSeries::Vix => "VIX",
            MacroSeries::Vix3m => "VIX3M",
            other => {
                return Err(SourceError::Decode(format!(
                    "{other:?} is not a CBOE index"
                )));
            }
        };
        let url = format!(
            "{}/api/global/us_indices/daily_prices/{index}_History.csv",
            self.base_url
        );
        let csv = self.http.fetch(self.http.client().get(url)).await?;
        parse_history_csv(series, &String::from_utf8_lossy(&csv))
    }
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
    let close_column = column(CLOSE_COLUMN)?;
    let mut closes = Vec::new();
    for record in reader.records() {
        let record = record.map_err(decode)?;
        let date_text = record.get(date_column).unwrap_or_default().trim();
        let date = NaiveDate::parse_from_str(date_text, "%m/%d/%Y")
            .map_err(|error| SourceError::Decode(format!("date {date_text:?}: {error}")))?;
        if series == MacroSeries::Vix && date < VIX_METHODOLOGY_START {
            continue;
        }
        let close_text = record.get(close_column).unwrap_or_default().trim();
        let close = close_text
            .parse::<Decimal>()
            .map_err(|error| SourceError::Decode(format!("close {close_text:?}: {error}")))?;
        closes.push(MacroObservation {
            series,
            date,
            published: date,
            value: Some(close),
        });
    }
    closes.sort_by_key(|close| close.date);
    Ok(closes)
}

fn decode(error: csv::Error) -> SourceError {
    SourceError::Decode(error.to_string())
}
