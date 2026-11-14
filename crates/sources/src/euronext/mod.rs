use std::time::Duration;

use buizz_core::market::{NordicBars, NordicListing, Symbol};
use chrono::NaiveDate;
use reqwest::header::CONTENT_TYPE;

use crate::http::{HttpClient, RateLimit, SourceError};
use crate::nordic::{BROWSER_USER_AGENT, DayRow, daily_bars};

pub const BASE_URL: &str = "https://live.euronext.com";
pub const URL_ENV: &str = "EURONEXT_URL";
pub const STOCKS_PATH: &str = "/en/pd_es/data/stocks/download";
pub const HISTORY_PATH: &str = "/en/ajax/AwlHistoricalPrice/getFullDownloadAjax";
pub const HISTORY_MONTHS: u32 = 24;
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 1,
    per: Duration::from_secs(2),
    cooldown: Duration::from_secs(300),
};
const OSLO_MICS: &str = "XOSL,MERK,XOAS";
const MARKETS: [(&str, &str); 3] = [
    ("Oslo Børs", "XOSL"),
    ("Euronext Expand Oslo", "XOAS"),
    ("Euronext Growth Oslo", "MERK"),
];
const LIST_FORM: &str =
    "args%5BinitialLetter%5D=&iDisplayLength=2000&iDisplayStart=0&sSortDir_0=asc&sSortField=name";
const BOM: char = '\u{feff}';
const QUERY_DATE: &str = "%Y-%m-%d";
const ROW_DATE: &str = "%d/%m/%Y";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EuronextConfig {
    pub base_url: String,
}

#[derive(Clone)]
pub struct EuronextClient {
    http: HttpClient,
    base_url: String,
}

impl EuronextClient {
    pub fn new(config: EuronextConfig) -> Result<Self, SourceError> {
        Ok(Self {
            http: HttpClient::new(BROWSER_USER_AGENT, RATE_LIMIT)?,
            base_url: config.base_url.trim_end_matches('/').to_owned(),
        })
    }

    pub async fn oslo_listings(&self) -> Result<Vec<NordicListing>, SourceError> {
        let request = self
            .http
            .client()
            .post(format!("{}{STOCKS_PATH}", self.base_url))
            .query(&[("mics", OSLO_MICS)])
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(LIST_FORM);
        normalize_listings(&String::from_utf8_lossy(&self.http.fetch(request).await?))
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
            .get(format!("{}{HISTORY_PATH}/{listing}", self.base_url))
            .query(&[
                ("format", "csv".to_owned()),
                ("decimal_separator", ".".to_owned()),
                ("date_form", "d/m/Y".to_owned()),
                ("op", String::new()),
                ("adjusted", "N".to_owned()),
                ("base100", String::new()),
                ("startdate", from.format(QUERY_DATE).to_string()),
                ("enddate", to.format(QUERY_DATE).to_string()),
            ]);
        normalize_history(&String::from_utf8_lossy(&self.http.fetch(request).await?))
    }
}

pub fn listing_id(isin: &str, mic: &str) -> Symbol {
    Symbol::new(&format!("{isin}-{mic}"))
}

fn decode(message: impl std::fmt::Display) -> SourceError {
    SourceError::Decode(format!("Euronext file: {message}"))
}

fn records(text: &str) -> Result<Vec<csv::StringRecord>, SourceError> {
    csv::ReaderBuilder::new()
        .delimiter(b';')
        .has_headers(false)
        .flexible(true)
        .from_reader(text.trim_start_matches(BOM).as_bytes())
        .records()
        .collect::<Result<_, _>>()
        .map_err(decode)
}

fn table(
    text: &str,
    first: &str,
    names: &[&str],
) -> Result<(Vec<usize>, Vec<csv::StringRecord>), SourceError> {
    let records = records(text)?;
    let header = records
        .iter()
        .position(|record| record.get(0).map(str::trim) == Some(first))
        .ok_or_else(|| decode(format!("no {first:?} header")))?;
    let columns = names
        .iter()
        .map(|name| {
            records[header]
                .iter()
                .position(|column| column.trim() == *name)
                .ok_or_else(|| decode(format!("no {name:?} column")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((columns, records.into_iter().skip(header + 1).collect()))
}

pub fn normalize_listings(text: &str) -> Result<Vec<NordicListing>, SourceError> {
    let (columns, rows) = table(
        text,
        "Name",
        &["Name", "ISIN", "Symbol", "Market", "Currency"],
    )?;
    let field = |row: &csv::StringRecord, index: usize| {
        row.get(columns[index])
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    Ok(rows
        .iter()
        .filter_map(|row| {
            let market = field(row, 3);
            let (_, mic) = MARKETS.iter().find(|(name, _)| *name == market)?;
            let isin = field(row, 1);
            (!isin.is_empty()).then(|| NordicListing {
                listing: listing_id(&isin, mic),
                mic: Some((*mic).to_owned()),
                symbol: field(row, 2),
                name: field(row, 0),
                currency: field(row, 4),
                sector: None,
                isin: Some(isin),
            })
        })
        .collect())
}

pub fn normalize_history(text: &str) -> Result<NordicBars, SourceError> {
    let (columns, rows) = table(
        text,
        "Date",
        &["Date", "Open", "High", "Low", "Close", "Number of Shares"],
    )?;
    let mut undated = 0;
    let mut daily = daily_bars(rows.iter().filter_map(|row| {
        let field = |index: usize| row.get(columns[index]).unwrap_or_default();
        let Ok(date) = NaiveDate::parse_from_str(field(0).trim(), ROW_DATE) else {
            undated += 1;
            return None;
        };
        Some(DayRow {
            date,
            open: field(1),
            high: field(2),
            low: field(3),
            close: field(4),
            volume: Some(field(5)),
        })
    }));
    daily.skipped += undated;
    Ok(daily)
}
