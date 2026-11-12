use std::time::Duration;

use buizz_core::market::RiskFreeRate;
use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::http::{HttpClient, RateLimit, SourceError};

pub const BASE_URL: &str = "https://home.treasury.gov";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 1,
    per: Duration::from_secs(1),
    cooldown: Duration::from_secs(60),
};
pub const URL_ENV: &str = "TREASURY_URL";
const DATE_COLUMN: &str = "Date";
const THREE_MONTH_COLUMN: &str = "3 Mo";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreasuryConfig {
    pub base_url: String,
}

impl TreasuryConfig {
    pub fn from_env() -> Self {
        Self {
            base_url: std::env::var(URL_ENV).unwrap_or_else(|_| BASE_URL.to_owned()),
        }
    }
}

impl Default for TreasuryConfig {
    fn default() -> Self {
        Self {
            base_url: BASE_URL.to_owned(),
        }
    }
}

#[derive(Clone)]
pub struct TreasuryClient {
    http: HttpClient,
    base_url: String,
}

impl TreasuryClient {
    pub fn new(config: TreasuryConfig) -> Result<Self, SourceError> {
        Ok(Self {
            http: HttpClient::new("buizz", RATE_LIMIT)?,
            base_url: config.base_url.trim_end_matches('/').to_owned(),
        })
    }

    pub async fn three_month_yields(&self, year: i32) -> Result<Vec<RiskFreeRate>, SourceError> {
        let url = format!(
            "{}/resource-center/data-chart-center/interest-rates/daily-treasury-rates.csv/{year}/all",
            self.base_url
        );
        let request = self.http.client().get(url).query(&[
            ("type", "daily_treasury_yield_curve"),
            ("field_tdr_date_value", &year.to_string()),
            ("_format", "csv"),
        ]);
        let csv = self.http.fetch(request).await?;
        parse_par_yield_csv(&String::from_utf8_lossy(&csv))
    }
}

pub fn parse_par_yield_csv(csv: &str) -> Result<Vec<RiskFreeRate>, SourceError> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(csv.as_bytes());
    let headers = reader.headers().map_err(decode)?.clone();
    let column = |name: &str| {
        headers
            .iter()
            .position(|header| header.trim() == name)
            .ok_or_else(|| SourceError::Decode(format!("missing {name:?} column")))
    };
    let date_column = column(DATE_COLUMN)?;
    let yield_column = column(THREE_MONTH_COLUMN)?;
    let mut rates = Vec::new();
    for record in reader.records() {
        let record = record.map_err(decode)?;
        let cell = record.get(yield_column).unwrap_or_default().trim();
        if cell.is_empty() {
            continue;
        }
        let date_text = record.get(date_column).unwrap_or_default().trim();
        let date = NaiveDate::parse_from_str(date_text, "%m/%d/%Y")
            .map_err(|error| SourceError::Decode(format!("date {date_text:?}: {error}")))?;
        let percent = cell
            .parse::<Decimal>()
            .map_err(|error| SourceError::Decode(format!("yield {cell:?}: {error}")))?;
        rates.push(RiskFreeRate { date, percent });
    }
    rates.sort_by_key(|rate| rate.date);
    Ok(rates)
}

fn decode(error: csv::Error) -> SourceError {
    SourceError::Decode(error.to_string())
}
