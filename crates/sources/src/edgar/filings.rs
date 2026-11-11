use buizz_core::market::{Company, EarningsRelease, Symbol};
use buizz_core::time::Timestamp;
use chrono::NaiveDate;
use serde::Deserialize;

use crate::http::SourceError;

const EARNINGS_FORMS: [&str; 2] = ["8-K", "8-K/A"];
const RESULTS_OF_OPERATIONS: &str = "2.02";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submissions {
    pub company: Company,
    pub releases: Vec<EarningsRelease>,
    pub older_pages: Vec<FilingsPage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct FilingsPage {
    pub name: String,
    #[serde(rename = "filingTo")]
    pub filing_to: NaiveDate,
}

#[derive(Deserialize)]
struct WireSubmissions {
    cik: String,
    sic: String,
    #[serde(rename = "sicDescription")]
    sic_description: Option<String>,
    filings: WireFilings,
}

#[derive(Deserialize)]
struct WireFilings {
    recent: Columns,
    #[serde(default)]
    files: Vec<FilingsPage>,
}

#[derive(Deserialize)]
struct Columns {
    #[serde(rename = "accessionNumber")]
    accession: Vec<String>,
    #[serde(rename = "filingDate")]
    filed: Vec<NaiveDate>,
    #[serde(rename = "reportDate")]
    report_date: Vec<String>,
    #[serde(rename = "acceptanceDateTime")]
    accepted: Vec<String>,
    form: Vec<String>,
    items: Vec<String>,
}

fn decode<T: for<'de> Deserialize<'de>>(payload: &[u8]) -> Result<T, SourceError> {
    serde_json::from_slice(payload).map_err(|error| SourceError::Decode(error.to_string()))
}

pub fn normalize_submissions(symbol: &Symbol, payload: &[u8]) -> Result<Submissions, SourceError> {
    let wire: WireSubmissions = decode(payload)?;
    let cik: u32 = wire
        .cik
        .parse()
        .map_err(|_| SourceError::Decode(format!("submissions CIK {:?}", wire.cik)))?;
    let sic = wire.sic.trim().parse().ok().filter(|code| *code > 0);
    Ok(Submissions {
        company: Company {
            symbol: symbol.clone(),
            cik,
            sic,
            sic_description: sic
                .and(wire.sic_description)
                .filter(|text| !text.is_empty()),
        },
        releases: earnings_releases(symbol, cik, &wire.filings.recent)?,
        older_pages: wire
            .filings
            .files
            .into_iter()
            .filter(|page| {
                page.name.ends_with(".json")
                    && page.name.chars().all(|letter| {
                        letter.is_ascii_alphanumeric() || letter == '-' || letter == '.'
                    })
            })
            .collect(),
    })
}

pub fn normalize_filings_page(
    symbol: &Symbol,
    cik: u32,
    payload: &[u8],
) -> Result<Vec<EarningsRelease>, SourceError> {
    earnings_releases(symbol, cik, &decode(payload)?)
}

fn earnings_releases(
    symbol: &Symbol,
    cik: u32,
    columns: &Columns,
) -> Result<Vec<EarningsRelease>, SourceError> {
    let rows = columns.accession.len();
    let lengths = [
        columns.filed.len(),
        columns.report_date.len(),
        columns.accepted.len(),
        columns.form.len(),
        columns.items.len(),
    ];
    if lengths.iter().any(|length| *length != rows) {
        return Err(SourceError::Decode(
            "submissions filing columns differ in length".to_owned(),
        ));
    }
    let mut releases = Vec::new();
    for row in 0..rows {
        let announces_results = EARNINGS_FORMS.contains(&columns.form[row].as_str())
            && columns.items[row]
                .split(',')
                .any(|item| item.trim() == RESULTS_OF_OPERATIONS);
        if !announces_results {
            continue;
        }
        let accepted_at: Timestamp = columns.accepted[row].parse().map_err(|_| {
            SourceError::Decode(format!(
                "acceptance time {:?} of {}",
                columns.accepted[row], columns.accession[row]
            ))
        })?;
        releases.push(EarningsRelease {
            symbol: symbol.clone(),
            cik,
            accession: columns.accession[row].clone(),
            form: columns.form[row].clone(),
            filed: columns.filed[row],
            report_date: columns.report_date[row].parse().ok(),
            accepted_at,
        });
    }
    Ok(releases)
}
