use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use buizz_core::market::{
    Calendar, Exhibit, FilingDocument, FundamentalFact, InsiderReport, PressExhibit, SecFiling,
    Symbol,
};
use buizz_core::time::Timestamp;
use chrono::{Datelike, NaiveDate};
use reqwest::RequestBuilder;
use serde::Deserialize;

use crate::http::{HttpClient, RateLimit, SourceError};

mod concepts;
mod exhibit;
mod filings;
mod insider_quarters;
mod normalize;
mod ownership;
mod sections;
mod submission;

pub use exhibit::{IndexedDocument, cover_tickers, exhibit_text, press_release_document};
pub use filings::{
    Filings, FilingsPage, OwnershipFiling, Submissions, normalize_filings_page,
    normalize_submissions,
};
pub use insider_quarters::{
    FIRST_INSIDER_QUARTER, INSIDER_QUARTERS_PAGE, InsiderQuarter, insider_quarter_links,
    normalize_insider_quarter,
};
pub use normalize::normalize_company_facts;
pub use ownership::normalize_ownership;
pub use sections::{SPLITTER_VERSION, split_sections};
use submission::submission_documents;

pub const DATA_URL: &str = "https://data.sec.gov";
pub const WWW_URL: &str = "https://www.sec.gov";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 8,
    per: Duration::from_secs(1),
    cooldown: Duration::from_secs(10 * 60),
};
pub const ACCEPTED_FORMS: [&str; 8] = [
    "10-K", "10-K/A", "10-Q", "10-Q/A", "20-F", "20-F/A", "40-F", "40-F/A",
];
pub const EARNINGS_FORMS: [&str; 2] = ["8-K", "8-K/A"];
pub const INSIDER_FORMS: [&str; 2] = ["4", "4/A"];
pub const PERIODIC_FORMS: [&str; 4] = ["10-K", "10-K/A", "10-Q", "10-Q/A"];
pub const COMPANY_FORMS: [&str; 6] = ["8-K", "8-K/A", "10-K", "10-K/A", "10-Q", "10-Q/A"];
pub const DELISTING_FORMS: [&str; 2] = ["25", "25-NSE"];
const NOT_FOUND: u16 = 404;
const REFUSED: u16 = 403;
const SUBMISSION_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgarConfig {
    pub user_agent: String,
    pub data_url: String,
    pub www_url: String,
}

impl EdgarConfig {
    pub fn new(user_agent: String) -> Self {
        Self {
            user_agent,
            data_url: DATA_URL.to_owned(),
            www_url: WWW_URL.to_owned(),
        }
    }
}

#[derive(Deserialize)]
struct DirectoryListing {
    directory: Directory,
}

#[derive(Deserialize)]
struct Directory {
    item: Vec<DirectoryItem>,
}

#[derive(Deserialize)]
struct DirectoryItem {
    name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitKind {
    Currency,
    Shares,
    CurrencyPerShare,
}

#[derive(Clone)]
pub struct EdgarClient {
    http: HttpClient,
    data_url: String,
    www_url: String,
}

#[derive(Deserialize)]
struct TickerEntry {
    cik_str: u32,
    ticker: String,
}

#[derive(Deserialize)]
struct FundTable {
    fields: Vec<String>,
    data: Vec<Vec<serde_json::Value>>,
}

fn symbol(ticker: &str) -> Symbol {
    Symbol::new(&ticker.replace('-', "."))
}

impl EdgarClient {
    pub fn new(config: EdgarConfig) -> Result<Self, SourceError> {
        Ok(Self {
            http: HttpClient::new(&config.user_agent, RATE_LIMIT)?,
            data_url: config.data_url.trim_end_matches('/').to_owned(),
            www_url: config.www_url.trim_end_matches('/').to_owned(),
        })
    }

    pub async fn ticker_map(&self) -> Result<BTreeMap<Symbol, u32>, SourceError> {
        let url = format!("{}/files/company_tickers.json", self.www_url);
        let body = self.fetch(url).await?;
        let entries: BTreeMap<String, TickerEntry> = serde_json::from_slice(&body)
            .map_err(|error| SourceError::Decode(error.to_string()))?;
        Ok(entries
            .into_values()
            .map(|entry| (symbol(&entry.ticker), entry.cik_str))
            .collect())
    }

    pub async fn fund_tickers(&self) -> Result<BTreeSet<Symbol>, SourceError> {
        let url = format!("{}/files/company_tickers_mf.json", self.www_url);
        let body = self.fetch(url).await?;
        let table: FundTable = serde_json::from_slice(&body)
            .map_err(|error| SourceError::Decode(error.to_string()))?;
        let column = table
            .fields
            .iter()
            .position(|field| field == "symbol")
            .ok_or_else(|| SourceError::Decode("fund tickers without a symbol field".to_owned()))?;
        Ok(table
            .data
            .iter()
            .filter_map(|row| row.get(column)?.as_str().map(symbol))
            .collect())
    }

    pub async fn company_facts(
        &self,
        symbol: &Symbol,
        cik: u32,
        calendar: &Calendar,
    ) -> Result<Vec<FundamentalFact>, SourceError> {
        let url = format!("{}/api/xbrl/companyfacts/CIK{cik:010}.json", self.data_url);
        let body = self.fetch(url).await?;
        normalize_company_facts(symbol, cik, &body, calendar)
    }

    pub async fn submissions(&self, symbol: &Symbol, cik: u32) -> Result<Submissions, SourceError> {
        let url = format!("{}/submissions/CIK{cik:010}.json", self.data_url);
        normalize_submissions(symbol, &self.fetch(url).await?)
    }

    pub async fn filings_page(
        &self,
        symbol: &Symbol,
        cik: u32,
        page: &FilingsPage,
    ) -> Result<Filings, SourceError> {
        let url = format!("{}/submissions/{}", self.data_url, page.name);
        normalize_filings_page(symbol, cik, &self.fetch(url).await?)
    }

    pub async fn ownership(
        &self,
        cik: u32,
        filing: &OwnershipFiling,
    ) -> Result<InsiderReport, SourceError> {
        let document = filing.document.rsplit('/').next().unwrap_or_default();
        let url = format!(
            "{}/Archives/edgar/data/{cik}/{}/{document}",
            self.www_url,
            filing.accession.replace('-', "")
        );
        normalize_ownership(&filing.accession, &self.fetch(url).await?)
    }

    pub async fn insider_quarters(&self) -> Result<Vec<InsiderQuarter>, SourceError> {
        let page = self
            .fetch(format!("{}{INSIDER_QUARTERS_PAGE}", self.www_url))
            .await?;
        Ok(insider_quarter_links(&String::from_utf8_lossy(&page)))
    }

    pub async fn insider_quarter(
        &self,
        quarter: &InsiderQuarter,
    ) -> Result<Vec<InsiderReport>, SourceError> {
        let payload = self
            .fetch(format!("{}{}", self.www_url, quarter.path))
            .await?;
        tokio::task::spawn_blocking(move || normalize_insider_quarter(&payload))
            .await
            .map_err(|error| SourceError::Decode(error.to_string()))?
    }

    pub async fn press_release(
        &self,
        cik: u32,
        accession: &str,
        accepted_at: Timestamp,
    ) -> Result<PressExhibit, SourceError> {
        let folder = format!(
            "{}/Archives/edgar/data/{cik}/{}",
            self.www_url,
            accession.replace('-', "")
        );
        let index = self
            .fetch(format!("{folder}/{accession}-index.htm"))
            .await?;
        let listed = press_release_document(&String::from_utf8_lossy(&index));
        let exhibit = match listed {
            Some(IndexedDocument { document, kind, .. }) => {
                let payload = self.fetch(format!("{folder}/{document}")).await?;
                let text = exhibit_text(&document, &payload);
                (!text.is_empty()).then_some(Exhibit {
                    document,
                    kind,
                    text,
                })
            }
            None => None,
        };
        Ok(PressExhibit {
            cik,
            accession: accession.to_owned(),
            accepted_at,
            exhibit,
        })
    }

    pub async fn filing_documents(
        &self,
        cik: u32,
        accession: &str,
    ) -> Result<Vec<FilingDocument>, SourceError> {
        let url = format!(
            "{}/Archives/edgar/data/{cik}/{}/{accession}.txt",
            self.www_url,
            accession.replace('-', "")
        );
        let request = self.http.client().get(url).timeout(SUBMISSION_TIMEOUT);
        submission_documents(accession, &self.send(request).await?)
    }

    pub async fn delisting_notices(
        &self,
        year: i32,
        quarter: u32,
    ) -> Result<Vec<SecFiling>, SourceError> {
        let url = format!(
            "{}/Archives/edgar/full-index/{year}/QTR{quarter}/master.idx",
            self.www_url
        );
        let mut notices = normalize_daily_index(&self.fetch(url).await?);
        notices.retain(|filing| DELISTING_FORMS.contains(&filing.form.as_str()));
        Ok(notices)
    }

    pub async fn report_tickers(
        &self,
        cik: u32,
        accession: &str,
    ) -> Result<Vec<Symbol>, SourceError> {
        let url = format!(
            "{}/Archives/edgar/data/{cik}/{}/R1.htm",
            self.www_url,
            accession.replace('-', "")
        );
        match self.fetch(url).await {
            Ok(body) => Ok(cover_tickers(&String::from_utf8_lossy(&body))),
            Err(SourceError::Status {
                status: NOT_FOUND, ..
            }) => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

    pub async fn daily_index(&self, day: NaiveDate) -> Result<Option<Vec<SecFiling>>, SourceError> {
        let quarter = format!(
            "{}/Archives/edgar/daily-index/{}/QTR{}",
            self.www_url,
            day.year(),
            day.month0() / 3 + 1
        );
        let file = format!("master.{}.idx", day.format("%Y%m%d"));
        match self.fetch(format!("{quarter}/{file}")).await {
            Ok(body) => Ok(Some(normalize_daily_index(&body))),
            Err(SourceError::Status {
                status: NOT_FOUND, ..
            }) => Ok(None),
            Err(refused @ SourceError::RateLimited { .. }) => {
                let listing: DirectoryListing =
                    serde_json::from_slice(&self.fetch(format!("{quarter}/index.json")).await?)
                        .map_err(|error| SourceError::Decode(error.to_string()))?;
                if listing.directory.item.iter().any(|item| item.name == file) {
                    Err(refused)
                } else {
                    Ok(None)
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn fetch(&self, url: String) -> Result<Vec<u8>, SourceError> {
        self.send(self.http.client().get(url)).await
    }

    async fn send(&self, request: RequestBuilder) -> Result<Vec<u8>, SourceError> {
        match self.http.fetch(request).await {
            Err(SourceError::Status {
                status: REFUSED, ..
            }) => Err(SourceError::RateLimited {
                retry_after: Some(RATE_LIMIT.cooldown),
            }),
            outcome => outcome,
        }
    }
}

pub fn normalize_daily_index(payload: &[u8]) -> Vec<SecFiling> {
    let text = String::from_utf8_lossy(payload);
    text.lines()
        .skip_while(|line| !line.starts_with("---"))
        .skip(1)
        .filter_map(|line| {
            let [cik, _, form, filed, file] = line.split('|').collect::<Vec<_>>()[..] else {
                return None;
            };
            let form = form.trim();
            if ![
                &ACCEPTED_FORMS[..],
                &EARNINGS_FORMS,
                &INSIDER_FORMS,
                &DELISTING_FORMS,
            ]
            .iter()
            .any(|forms| forms.contains(&form))
            {
                return None;
            }
            let accession = file.trim().rsplit('/').next()?.strip_suffix(".txt")?;
            Some(SecFiling {
                cik: cik.trim().parse().ok()?,
                accession: accession.to_owned(),
                form: form.to_owned(),
                filed: NaiveDate::parse_from_str(filed.trim(), "%Y%m%d")
                    .or_else(|_| NaiveDate::parse_from_str(filed.trim(), "%Y-%m-%d"))
                    .ok()?,
            })
        })
        .collect()
}
