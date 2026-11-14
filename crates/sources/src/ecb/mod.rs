use std::str::FromStr;
use std::time::Duration;

use buizz_core::market::FxRate;
use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::http::{HttpClient, RateLimit, SourceError};

pub const BASE_URL: &str = "https://data-api.ecb.europa.eu";
pub const URL_ENV: &str = "ECB_URL";
pub const EXR_PATH: &str = "/service/data/EXR";
pub const CURRENCIES: [&str; 5] = ["SEK", "NOK", "DKK", "ISK", "USD"];
pub const FIRST_RATE: NaiveDate = NaiveDate::from_ymd_opt(1999, 1, 4).expect("a date");
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 1,
    per: Duration::from_secs(1),
    cooldown: Duration::from_secs(300),
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EcbConfig {
    pub base_url: String,
}

#[derive(Clone)]
pub struct EcbClient {
    http: HttpClient,
    base_url: String,
}

impl EcbClient {
    pub fn new(config: EcbConfig) -> Result<Self, SourceError> {
        Ok(Self {
            http: HttpClient::new("buizz", RATE_LIMIT)?,
            base_url: config.base_url.trim_end_matches('/').to_owned(),
        })
    }

    pub async fn euro_rates(&self, since: NaiveDate) -> Result<Vec<FxRate>, SourceError> {
        let request = self
            .http
            .client()
            .get(format!(
                "{}{EXR_PATH}/D.{}.EUR.SP00.A",
                self.base_url,
                CURRENCIES.join("+")
            ))
            .query(&[
                ("startPeriod", since.to_string()),
                ("format", "csvdata".to_owned()),
            ]);
        parse_rates(&String::from_utf8_lossy(&self.http.fetch(request).await?))
    }
}

fn decode(message: impl std::fmt::Display) -> SourceError {
    SourceError::Decode(format!("ECB exchange rates: {message}"))
}

pub fn parse_rates(text: &str) -> Result<Vec<FxRate>, SourceError> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut reader = csv::Reader::from_reader(text.as_bytes());
    let headers = reader.headers().map_err(decode)?.clone();
    let column = |name: &str| {
        headers
            .iter()
            .position(|header| header == name)
            .ok_or_else(|| decode(format!("no {name} column")))
    };
    let (currency, period, value) = (
        column("CURRENCY")?,
        column("TIME_PERIOD")?,
        column("OBS_VALUE")?,
    );
    let mut rates = Vec::new();
    for record in reader.records() {
        let record = record.map_err(decode)?;
        let field = |index: usize| record.get(index).unwrap_or_default().trim();
        let Ok(per_euro) = Decimal::from_str(field(value)) else {
            continue;
        };
        rates.push(FxRate {
            currency: field(currency).to_owned(),
            date: NaiveDate::from_str(field(period)).map_err(decode)?,
            per_euro,
        });
    }
    Ok(rates)
}
