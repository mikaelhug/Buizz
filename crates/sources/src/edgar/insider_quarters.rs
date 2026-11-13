use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::sync::LazyLock;

use buizz_core::market::{InsiderOwner, InsiderReport, InsiderTransaction};
use chrono::NaiveDate;
use csv::{ByteRecord, ReaderBuilder};
use regex::Regex;
use zip::ZipArchive;

use super::INSIDER_FORMS;
use super::ownership::{amount, truthy};
use crate::http::SourceError;

pub const INSIDER_QUARTERS_PAGE: &str =
    "/data-research/sec-markets-data/insider-transactions-data-sets";
pub const FIRST_INSIDER_QUARTER: (i32, u32) = (2006, 1);

static QUARTER_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"href="(/files/[^"]+/(\d{4})q([1-4])_form345\.zip)""#)
        .expect("the data set link pattern compiles")
});

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InsiderQuarter {
    pub quarter: (i32, u32),
    pub path: String,
}

pub fn insider_quarter_links(page: &str) -> Vec<InsiderQuarter> {
    let mut links: Vec<InsiderQuarter> = QUARTER_LINK
        .captures_iter(page)
        .filter_map(|link| {
            Some(InsiderQuarter {
                quarter: (link[2].parse().ok()?, link[3].parse().ok()?),
                path: link[1].to_owned(),
            })
        })
        .collect();
    links.sort_by_key(|link| link.quarter);
    links.dedup_by_key(|link| link.quarter);
    links
}

struct Table<'a> {
    columns: &'a BTreeMap<String, usize>,
    record: &'a ByteRecord,
}

impl Table<'_> {
    fn get(&self, column: &str) -> Option<String> {
        let field = self.record.get(*self.columns.get(column)?)?;
        let text = String::from_utf8_lossy(field).trim().to_owned();
        (!text.is_empty()).then_some(text)
    }
}

fn undecodable(file: &str, error: &dyn std::fmt::Display) -> SourceError {
    SourceError::Decode(format!("insider data set {file}: {error}"))
}

fn rows(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    file: &str,
    required: &[&str],
    mut each: impl FnMut(&Table<'_>),
) -> Result<(), SourceError> {
    let mut payload = Vec::new();
    archive
        .by_name(file)
        .map_err(|error| undecodable(file, &error))?
        .read_to_end(&mut payload)
        .map_err(|error| undecodable(file, &error))?;
    let mut reader = ReaderBuilder::new()
        .delimiter(b'\t')
        .flexible(true)
        .from_reader(payload.as_slice());
    let columns: BTreeMap<String, usize> = reader
        .byte_headers()
        .map_err(|error| undecodable(file, &error))?
        .iter()
        .enumerate()
        .map(|(index, name)| (String::from_utf8_lossy(name).trim().to_owned(), index))
        .collect();
    if let Some(missing) = required.iter().find(|name| !columns.contains_key(**name)) {
        return Err(undecodable(file, &format!("no {missing} column")));
    }
    let mut record = ByteRecord::new();
    while reader
        .read_byte_record(&mut record)
        .map_err(|error| undecodable(file, &error))?
    {
        each(&Table {
            columns: &columns,
            record: &record,
        });
    }
    Ok(())
}

fn owner(row: &Table<'_>) -> Option<InsiderOwner> {
    let relationship = row.get("RPTOWNER_RELATIONSHIP").unwrap_or_default();
    let holds = |role: &str| {
        relationship
            .split(',')
            .any(|held| held.trim().eq_ignore_ascii_case(role))
    };
    Some(InsiderOwner {
        cik: row.get("RPTOWNERCIK")?.parse().ok()?,
        name: row.get("RPTOWNERNAME").unwrap_or_default(),
        director: holds("Director"),
        officer: holds("Officer"),
        ten_percent_owner: holds("TenPercentOwner"),
        other: holds("Other"),
        officer_title: row.get("RPTOWNER_TITLE"),
    })
}

fn transaction(row: &Table<'_>) -> Option<(u64, InsiderTransaction)> {
    let line = row.get("NONDERIV_TRANS_SK")?.parse().ok()?;
    Some((
        line,
        InsiderTransaction {
            security: row.get("SECURITY_TITLE").unwrap_or_default(),
            date: NaiveDate::parse_from_str(&row.get("TRANS_DATE")?, "%d-%b-%Y").ok()?,
            code: row.get("TRANS_CODE")?,
            acquired: row.get("TRANS_ACQUIRED_DISP_CD")? == "A",
            shares: amount(row.get("TRANS_SHARES")),
            price: amount(row.get("TRANS_PRICEPERSHARE")),
            shares_after: amount(row.get("SHRS_OWND_FOLWNG_TRANS")),
            direct: row
                .get("DIRECT_INDIRECT_OWNERSHIP")
                .is_none_or(|nature| nature == "D"),
        },
    ))
}

pub fn normalize_insider_quarter(payload: &[u8]) -> Result<Vec<InsiderReport>, SourceError> {
    let mut archive =
        ZipArchive::new(Cursor::new(payload)).map_err(|error| undecodable("zip", &error))?;
    let mut reports: BTreeMap<String, (InsiderReport, Vec<(u64, InsiderTransaction)>)> =
        BTreeMap::new();
    rows(
        &mut archive,
        "SUBMISSION.tsv",
        &["ACCESSION_NUMBER", "DOCUMENT_TYPE", "ISSUERCIK"],
        |row| {
            let report = || {
                let form = row.get("DOCUMENT_TYPE")?;
                INSIDER_FORMS.contains(&form.as_str()).then_some(())?;
                Some(InsiderReport {
                    accession: row.get("ACCESSION_NUMBER")?,
                    issuer_cik: row.get("ISSUERCIK")?.parse().ok()?,
                    plan_10b5_1: row.get("AFF10B5ONE").map(|flag| truthy(&flag)),
                    owners: Vec::new(),
                    transactions: Vec::new(),
                })
            };
            if let Some(report) = report() {
                reports.insert(report.accession.clone(), (report, Vec::new()));
            }
        },
    )?;
    rows(
        &mut archive,
        "REPORTINGOWNER.tsv",
        &["ACCESSION_NUMBER", "RPTOWNERCIK"],
        |row| {
            if let Some((report, _)) = row
                .get("ACCESSION_NUMBER")
                .and_then(|accession| reports.get_mut(&accession))
                && let Some(owner) = owner(row)
            {
                report.owners.push(owner);
            }
        },
    )?;
    rows(
        &mut archive,
        "NONDERIV_TRANS.tsv",
        &["ACCESSION_NUMBER", "NONDERIV_TRANS_SK", "TRANS_DATE"],
        |row| {
            if let Some((_, lines)) = row
                .get("ACCESSION_NUMBER")
                .and_then(|accession| reports.get_mut(&accession))
                && let Some(line) = transaction(row)
            {
                lines.push(line);
            }
        },
    )?;
    Ok(reports
        .into_values()
        .map(|(mut report, mut lines)| {
            lines.sort_by_key(|(line, _)| *line);
            report.transactions = lines.into_iter().map(|(_, trade)| trade).collect();
            report
        })
        .collect())
}
