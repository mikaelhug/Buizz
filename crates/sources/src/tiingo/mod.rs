use std::collections::HashMap;
use std::fmt;
use std::ops::RangeInclusive;
use std::time::Duration;

use buizz_core::market::{
    Bar, Calendar, CorporateAction, CorporateActionKind, Feed, Symbol, split_rates,
};
use buizz_core::time::Timestamp;
use chrono::NaiveDate;
use reqwest::header::AUTHORIZATION;
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer};

use crate::http::{HttpClient, REDACTED, RateLimit, SourceError, decimal};

pub const BASE_URL: &str = "https://api.tiingo.com";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 35,
    per: Duration::from_secs(60 * 60),
    cooldown: Duration::from_secs(60 * 60),
};
pub const REQUESTS_PER_DAY: u32 = 700;
pub const SYMBOLS_PER_MONTH: u32 = 350;
pub const KEY_ENV: &str = "TIINGO_API_KEY";
pub const URL_ENV: &str = "TIINGO_URL";
const MIN_OVERLAP: usize = 20;
const MAX_MEDIAN_DIFFERENCE: Decimal = Decimal::from_parts(1, 0, 0, false, 2);
const VOLUME_RATIOS: RangeInclusive<Decimal> =
    Decimal::from_parts(5, 0, 0, false, 1)..=Decimal::from_parts(2, 0, 0, false, 0);

#[derive(Clone, PartialEq, Eq)]
pub struct TiingoConfig {
    pub api_key: String,
    pub base_url: String,
}

impl fmt::Debug for TiingoConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TiingoConfig")
            .field("api_key", &REDACTED)
            .field("base_url", &self.base_url)
            .finish()
    }
}

impl TiingoConfig {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: BASE_URL.to_owned(),
        }
    }

    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var(KEY_ENV).ok()?.trim().to_owned();
        if api_key.is_empty() {
            return None;
        }
        let mut config = Self::new(api_key);
        if let Ok(base_url) = std::env::var(URL_ENV) {
            config.base_url = base_url;
        }
        Some(config)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TiingoDaily {
    pub date: NaiveDate,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: u64,
    pub div_cash: Decimal,
    pub split_factor: Decimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverlapVerdict {
    Trusted(Agreement),
    Insufficient { overlap: usize },
    Rejected { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agreement {
    pub overlap: usize,
    pub close_difference: Decimal,
    pub volume_ratio: Decimal,
}

impl fmt::Display for Agreement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} shared sessions, median close difference {}%, median volume ratio {}",
            self.overlap,
            (self.close_difference * Decimal::ONE_HUNDRED).round_dp(3),
            self.volume_ratio.round_dp(3)
        )
    }
}

#[derive(Clone)]
pub struct TiingoClient {
    http: HttpClient,
    base_url: String,
    api_key: String,
}

impl TiingoClient {
    pub fn new(config: TiingoConfig) -> Result<Self, SourceError> {
        Ok(Self {
            http: HttpClient::new("buizz", RATE_LIMIT)?,
            base_url: config.base_url.trim_end_matches('/').to_owned(),
            api_key: config.api_key,
        })
    }

    pub async fn daily(
        &self,
        symbol: &Symbol,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<TiingoDaily>, SourceError> {
        let url = format!(
            "{}/tiingo/daily/{}/prices",
            self.base_url,
            symbol.as_str().replace('.', "-")
        );
        let request = self
            .http
            .client()
            .get(url)
            .header(AUTHORIZATION, format!("Token {}", self.api_key))
            .query(&[("startDate", from.to_string()), ("endDate", to.to_string())]);
        let body = self.http.fetch(request).await?;
        let rows: Vec<Row> = serde_json::from_slice(&body)
            .map_err(|error| SourceError::Decode(error.to_string()))?;
        rows.into_iter().filter_map(Row::into_daily).collect()
    }
}

#[derive(Clone, Copy, Default)]
struct Amount(Decimal);

impl<'de> Deserialize<'de> for Amount {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        decimal(deserializer).map(Self)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    date: String,
    open: Amount,
    high: Amount,
    low: Amount,
    close: Option<Amount>,
    volume: u64,
    #[serde(default)]
    div_cash: Amount,
    #[serde(default = "no_split")]
    split_factor: Amount,
}

fn no_split() -> Amount {
    Amount(Decimal::ONE)
}

impl Row {
    fn into_daily(self) -> Option<Result<TiingoDaily, SourceError>> {
        let close = self.close?;
        let day = self.date.get(..10).unwrap_or(&self.date);
        Some(
            NaiveDate::parse_from_str(day, "%Y-%m-%d")
                .map(|date| TiingoDaily {
                    date,
                    open: self.open.0,
                    high: self.high.0,
                    low: self.low.0,
                    close: close.0,
                    volume: self.volume,
                    div_cash: self.div_cash.0,
                    split_factor: self.split_factor.0,
                })
                .map_err(|error| SourceError::Decode(format!("date {:?}: {error}", self.date))),
        )
    }
}

pub fn normalize(
    symbol: &Symbol,
    rows: &[TiingoDaily],
    calendar: &Calendar,
) -> (Vec<Bar>, Vec<CorporateAction>) {
    let mut bars = Vec::with_capacity(rows.len());
    let mut actions = Vec::new();
    for row in rows {
        if row.split_factor > Decimal::ZERO && row.split_factor != Decimal::ONE {
            let (old_rate, new_rate) = split_rates(row.split_factor);
            actions.push(CorporateAction {
                symbol: symbol.clone(),
                ex_date: row.date,
                kind: CorporateActionKind::Split { old_rate, new_rate },
            });
        }
        if row.div_cash > Decimal::ZERO {
            actions.push(CorporateAction {
                symbol: symbol.clone(),
                ex_date: row.date,
                kind: CorporateActionKind::CashDividend { rate: row.div_cash },
            });
        }
        if let Some(session) = calendar.session(row.date) {
            bars.push(Bar {
                symbol: symbol.clone(),
                start: session.open,
                open: row.open,
                high: row.high,
                low: row.low,
                close: row.close,
                volume: row.volume,
                feed: Feed::Sip,
            });
        }
    }
    (bars, actions)
}

pub fn validate_overlap(primary: &[Bar], secondary: &[Bar]) -> OverlapVerdict {
    let reference: HashMap<Timestamp, &Bar> = primary
        .iter()
        .filter(|bar| !bar.close.is_zero())
        .map(|bar| (bar.start, bar))
        .collect();
    let shared: Vec<(&Bar, &Bar)> = secondary
        .iter()
        .filter_map(|bar| Some((*reference.get(&bar.start)?, bar)))
        .collect();
    let overlap = shared.len();
    if overlap < MIN_OVERLAP {
        return OverlapVerdict::Insufficient { overlap };
    }
    let agreement = Agreement {
        overlap,
        close_difference: median(
            shared
                .iter()
                .map(|(primary, secondary)| (secondary.close - primary.close).abs() / primary.close)
                .collect(),
        ),
        volume_ratio: median(
            shared
                .iter()
                .filter(|(primary, _)| primary.volume > 0)
                .map(|(primary, secondary)| {
                    Decimal::from(secondary.volume) / Decimal::from(primary.volume)
                })
                .collect(),
        ),
    };
    if agreement.close_difference > MAX_MEDIAN_DIFFERENCE
        || !VOLUME_RATIOS.contains(&agreement.volume_ratio)
    {
        return OverlapVerdict::Rejected {
            reason: format!(
                "{agreement}, beyond {}% or outside {}..={}",
                MAX_MEDIAN_DIFFERENCE * Decimal::ONE_HUNDRED,
                VOLUME_RATIOS.start(),
                VOLUME_RATIOS.end()
            ),
        };
    }
    OverlapVerdict::Trusted(agreement)
}

fn median(mut values: Vec<Decimal>) -> Decimal {
    values.sort();
    let middle = values.len() / 2;
    match values.len() {
        0 => Decimal::ZERO,
        count if count % 2 == 1 => values[middle],
        _ => (values[middle - 1] + values[middle]) / Decimal::TWO,
    }
}
