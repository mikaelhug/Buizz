use buizz_core::market::{Company, EarningsRelease, Filing, Symbol};
use buizz_core::time::Timestamp;
use chrono::NaiveDate;
use serde::Deserialize;

use super::{EARNINGS_FORMS, INSIDER_FORMS, PERIODIC_FORMS, symbol as ticker};
use crate::http::{SourceError, decode_json};

const RESULTS_OF_OPERATIONS: &str = "2.02";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submissions {
    pub company: Company,
    pub filings: Filings,
    pub older_pages: Vec<FilingsPage>,
    pub tickers: Vec<Symbol>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filings {
    pub reports: Vec<Filing>,
    pub periodic: Vec<Filing>,
    pub releases: Vec<EarningsRelease>,
    pub ownership: Vec<OwnershipFiling>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnershipFiling {
    pub accession: String,
    pub form: String,
    pub filed: NaiveDate,
    pub accepted_at: Timestamp,
    pub document: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct FilingsPage {
    pub name: String,
    #[serde(rename = "filingTo")]
    pub filing_to: NaiveDate,
}

impl OwnershipFiling {
    pub fn is_xml(&self) -> bool {
        self.document.ends_with(".xml")
    }
}

#[derive(Deserialize)]
struct WireSubmissions {
    cik: String,
    sic: String,
    #[serde(rename = "sicDescription")]
    sic_description: Option<String>,
    #[serde(default)]
    tickers: Vec<String>,
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
    #[serde(rename = "primaryDocument", default)]
    document: Vec<String>,
}

pub fn normalize_submissions(symbol: &Symbol, payload: &[u8]) -> Result<Submissions, SourceError> {
    let wire: WireSubmissions = decode_json(payload)?;
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
        filings: filings(symbol, cik, &wire.filings.recent)?,
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
        tickers: wire.tickers.iter().map(|text| ticker(text)).collect(),
    })
}

pub fn normalize_filings_page(
    symbol: &Symbol,
    cik: u32,
    payload: &[u8],
) -> Result<Filings, SourceError> {
    filings(symbol, cik, &decode_json(payload)?)
}

fn filings(symbol: &Symbol, cik: u32, columns: &Columns) -> Result<Filings, SourceError> {
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
    let mut filings = Filings::default();
    for row in 0..rows {
        let accepted_at = || -> Result<Timestamp, SourceError> {
            columns.accepted[row].parse().map_err(|_| {
                SourceError::Decode(format!(
                    "acceptance time {:?} of {}",
                    columns.accepted[row], columns.accession[row]
                ))
            })
        };
        if INSIDER_FORMS.contains(&columns.form[row].as_str()) {
            filings.ownership.push(OwnershipFiling {
                accession: columns.accession[row].clone(),
                form: columns.form[row].clone(),
                filed: columns.filed[row],
                accepted_at: accepted_at()?,
                document: columns.document.get(row).cloned().unwrap_or_default(),
            });
            continue;
        }
        if PERIODIC_FORMS.contains(&columns.form[row].as_str()) {
            let accepted_at = accepted_at()?;
            filings.periodic.push(Filing {
                symbol: symbol.clone(),
                cik,
                accession: columns.accession[row].clone(),
                form: columns.form[row].clone(),
                filed: columns.filed[row],
                report_date: columns.report_date[row].parse().ok(),
                items: Vec::new(),
                accepted_at,
                available_at: accepted_at,
            });
            continue;
        }
        if !EARNINGS_FORMS.contains(&columns.form[row].as_str()) {
            continue;
        }
        let items: Vec<String> = columns.items[row]
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_owned)
            .collect();
        let accepted_at = accepted_at()?;
        let report_date = columns.report_date[row].parse().ok();
        if items.iter().any(|item| item == RESULTS_OF_OPERATIONS) {
            filings.releases.push(EarningsRelease {
                symbol: symbol.clone(),
                cik,
                accession: columns.accession[row].clone(),
                form: columns.form[row].clone(),
                filed: columns.filed[row],
                report_date,
                accepted_at,
            });
        }
        filings.reports.push(Filing {
            symbol: symbol.clone(),
            cik,
            accession: columns.accession[row].clone(),
            form: columns.form[row].clone(),
            filed: columns.filed[row],
            report_date,
            items,
            accepted_at,
            available_at: accepted_at,
        });
    }
    Ok(filings)
}
