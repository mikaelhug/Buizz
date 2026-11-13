use std::io::{Cursor, Read};
use std::sync::LazyLock;

use buizz_core::market::{FailToDeliverRecord, Symbol};
use buizz_core::visibility::{FAILS_HALF_MONTHS_FROM, fails_to_deliver_published};
use chrono::{Datelike, Months, NaiveDate, TimeDelta};
use regex::Regex;
use rust_decimal::Decimal;

use super::EdgarClient;
use crate::http::SourceError;

pub const FAILS_PAGE: &str = "/data-research/sec-markets-data/fails-deliver-data";
const HEADER: &str = "SETTLEMENT DATE|CUSIP|SYMBOL|QUANTITY (FAILS)|DESCRIPTION|PRICE";
const TRAILER: &str = "Trailer ";
const NO_PRICE: &str = ".";
const FIRST_FAILS_QUARTER: NaiveDate = NaiveDate::from_ymd_opt(2004, 1, 1).expect("valid date");
const SECOND_HALF_STARTS: u32 = 16;

static FILE_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"href="(?:https?://[^/"]+)?(/files/data/[^"]*?/((?:cnsfails\d{6}[ab])|(?:cnsp_sec_fails_\d{4}q[1-4]))\.zip)""#)
        .expect("valid regex")
});

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FailsFile {
    pub name: String,
    pub path: String,
}

pub struct FailsArchive {
    pub records: Vec<FailToDeliverRecord>,
    pub malformed_rows: usize,
}

impl EdgarClient {
    pub async fn fails_files(&self) -> Result<Vec<FailsFile>, SourceError> {
        let page = self.fetch(format!("{}{FAILS_PAGE}", self.www_url)).await?;
        Ok(parse_fails_page(&String::from_utf8_lossy(&page)))
    }

    pub async fn fails_to_deliver(
        &self,
        file: &FailsFile,
        downloaded: NaiveDate,
    ) -> Result<FailsArchive, SourceError> {
        let archive = self.fetch(format!("{}{}", self.www_url, file.path)).await?;
        parse_fails_archive(&archive, downloaded)
    }
}

pub fn fails_file_settlements(name: &str) -> Option<(NaiveDate, NaiveDate)> {
    let month_end = |start: NaiveDate, months: u32| {
        start
            .checked_add_months(Months::new(months))
            .map(|next| next - TimeDelta::days(1))
    };
    if let Some(period) = name.strip_prefix("cnsp_sec_fails_") {
        let (year, quarter) = period.split_once('q')?;
        let quarter: u32 = quarter.parse().ok()?;
        let start = NaiveDate::from_ymd_opt(year.parse().ok()?, quarter * 3 - 2, 1)?;
        return Some((start, month_end(start, 3)?));
    }
    let period = name.strip_prefix("cnsfails")?;
    let (month, half) = period.split_at_checked(6)?;
    let first = NaiveDate::parse_from_str(&format!("{month}01"), "%Y%m%d").ok()?;
    match half {
        "a" => Some((first, first.with_day(SECOND_HALF_STARTS - 1)?)),
        "b" => Some((first.with_day(SECOND_HALF_STARTS)?, month_end(first, 1)?)),
        _ => None,
    }
}

pub fn published_fails_files(today: NaiveDate) -> Vec<String> {
    let quarters = (0..)
        .map_while(|step| FIRST_FAILS_QUARTER.checked_add_months(Months::new(3 * step)))
        .take_while(|start| *start < FAILS_HALF_MONTHS_FROM)
        .map(|start| format!("cnsp_sec_fails_{}q{}", start.year(), start.month0() / 3 + 1));
    let halves = (0..)
        .map_while(|step| FAILS_HALF_MONTHS_FROM.checked_add_months(Months::new(step)))
        .take_while(|month| *month <= today)
        .flat_map(|month| ["a", "b"].map(|half| format!("cnsfails{}{half}", month.format("%Y%m"))));
    quarters
        .chain(halves)
        .filter(|name| {
            fails_file_settlements(name)
                .is_some_and(|(_, last)| fails_to_deliver_published(last, NaiveDate::MAX) <= today)
        })
        .collect()
}

pub fn parse_fails_page(html: &str) -> Vec<FailsFile> {
    let mut files: Vec<FailsFile> = FILE_LINK
        .captures_iter(html)
        .map(|link| FailsFile {
            name: link[2].to_owned(),
            path: link[1].to_owned(),
        })
        .collect();
    files.sort_by_key(|file| (fails_file_settlements(&file.name), file.name.clone()));
    files.dedup_by(|a, b| a.name == b.name);
    files
}

pub fn parse_fails_archive(
    archive: &[u8],
    downloaded: NaiveDate,
) -> Result<FailsArchive, SourceError> {
    let decode = |error: zip::result::ZipError| SourceError::Decode(error.to_string());
    let mut zip = zip::ZipArchive::new(Cursor::new(archive)).map_err(decode)?;
    let mut parsed = FailsArchive {
        records: Vec::new(),
        malformed_rows: 0,
    };
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(decode)?;
        if !entry.is_file() {
            continue;
        }
        let name = entry.name().map_err(decode)?.into_owned();
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| SourceError::Decode(format!("{name}: {error}")))?;
        let text = String::from_utf8_lossy(&bytes);
        let mut lines = text.lines().map(str::trim_end);
        if lines.next() != Some(HEADER) {
            return Err(SourceError::Decode(format!(
                "{name} does not start with the fails-to-deliver header"
            )));
        }
        for line in lines.filter(|line| !line.is_empty() && !line.starts_with(TRAILER)) {
            match parse_row(line, downloaded) {
                Some(record) => parsed.records.push(record),
                None => parsed.malformed_rows += 1,
            }
        }
    }
    Ok(parsed)
}

fn parse_row(line: &str, downloaded: NaiveDate) -> Option<FailToDeliverRecord> {
    let fields: Vec<&str> = line.split('|').map(str::trim).collect();
    let [settlement, cusip, symbol, quantity, .., price] = fields[..] else {
        return None;
    };
    if symbol.is_empty() || cusip.is_empty() {
        return None;
    }
    let settlement = NaiveDate::parse_from_str(settlement, "%Y%m%d").ok()?;
    Some(FailToDeliverRecord {
        symbol: Symbol::new(symbol),
        settlement,
        cusip: cusip.to_owned(),
        quantity: quantity.parse().ok()?,
        price: match price {
            NO_PRICE | "" => None,
            text => Some(text.parse::<Decimal>().ok()?),
        },
        published: fails_to_deliver_published(settlement, downloaded),
    })
}
