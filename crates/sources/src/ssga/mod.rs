use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::time::Duration;

use buizz_core::market::{EtfHoldingRecord, EtfHoldingsFile, EtfSharesRecord, Symbol};
use chrono::NaiveDate;
use roxmltree::Document;
use rust_decimal::Decimal;

use crate::http::{HttpClient, RateLimit, SourceError};

pub const BASE_URL: &str = "https://www.ssga.com";
pub const URL_ENV: &str = "SSGA_URL";
pub const FUND_DATA_PATH: &str = "/library-content/products/fund-data/etfs/us";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 2,
    per: Duration::from_secs(1),
    cooldown: Duration::from_secs(60),
};
pub const ETFS: [&str; 15] = [
    "SPY", "DIA", "MDY", "BIL", "XLB", "XLC", "XLE", "XLF", "XLI", "XLK", "XLP", "XLRE", "XLU",
    "XLV", "XLY",
];
const SHEET: &str = "xl/worksheets/sheet1.xml";
const SHARED_STRINGS: &str = "xl/sharedStrings.xml";
const DATE_FORMAT: &str = "%d-%b-%Y";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SsgaConfig {
    pub base_url: String,
}

#[derive(Clone)]
pub struct SsgaClient {
    http: HttpClient,
    base_url: String,
}

impl SsgaClient {
    pub fn new(config: SsgaConfig) -> Result<Self, SourceError> {
        Ok(Self {
            http: HttpClient::new("buizz", RATE_LIMIT)?,
            base_url: config.base_url.trim_end_matches('/').to_owned(),
        })
    }

    pub async fn holdings(&self, etf: &Symbol) -> Result<EtfHoldingsFile, SourceError> {
        let rows = self.workbook("holdings-daily", etf).await?;
        normalize_holdings(etf, &rows)
    }

    pub async fn shares(&self, etf: &Symbol) -> Result<Vec<EtfSharesRecord>, SourceError> {
        let rows = self.workbook("navhist", etf).await?;
        normalize_shares(etf, &rows)
    }

    async fn workbook(&self, kind: &str, etf: &Symbol) -> Result<Vec<Vec<String>>, SourceError> {
        let url = format!(
            "{}{FUND_DATA_PATH}/{kind}-us-en-{}.xlsx",
            self.base_url,
            etf.as_str().to_ascii_lowercase()
        );
        let body = self.http.fetch(self.http.client().get(url)).await?;
        read_sheet(&body)
    }
}

fn decode(message: impl std::fmt::Display) -> SourceError {
    SourceError::Decode(format!("SSGA workbook: {message}"))
}

fn entry(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<String, SourceError> {
    let mut text = String::new();
    archive
        .by_name(name)
        .map_err(decode)?
        .read_to_string(&mut text)
        .map_err(decode)?;
    Ok(text)
}

fn column(reference: &str) -> usize {
    reference
        .bytes()
        .take_while(u8::is_ascii_uppercase)
        .fold(0, |index, letter| {
            index * 26 + usize::from(letter - b'A') + 1
        })
        .saturating_sub(1)
}

pub fn read_sheet(xlsx: &[u8]) -> Result<Vec<Vec<String>>, SourceError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(xlsx)).map_err(decode)?;
    let strings_xml = entry(&mut archive, SHARED_STRINGS)?;
    let sheet_xml = entry(&mut archive, SHEET)?;
    let strings_doc = Document::parse(&strings_xml).map_err(decode)?;
    let strings: Vec<String> = strings_doc
        .root_element()
        .children()
        .filter(|node| node.has_tag_name("si"))
        .map(|item| {
            item.descendants()
                .filter(|node| node.has_tag_name("t"))
                .filter_map(|node| node.text())
                .collect()
        })
        .collect();
    let sheet = Document::parse(&sheet_xml).map_err(decode)?;
    let mut rows = Vec::new();
    for row in sheet.descendants().filter(|node| node.has_tag_name("row")) {
        let mut cells: BTreeMap<usize, String> = BTreeMap::new();
        for cell in row.children().filter(|node| node.has_tag_name("c")) {
            let Some(value) = cell
                .children()
                .find(|node| node.has_tag_name("v"))
                .and_then(|node| node.text())
            else {
                continue;
            };
            let text = if cell.attribute("t") == Some("s") {
                let index: usize = value.parse().map_err(decode)?;
                strings
                    .get(index)
                    .cloned()
                    .ok_or_else(|| decode(format!("shared string {index}")))?
            } else {
                value.to_owned()
            };
            cells.insert(column(cell.attribute("r").unwrap_or_default()), text);
        }
        let width = cells.keys().next_back().map_or(0, |last| last + 1);
        rows.push(
            (0..width)
                .map(|index| cells.remove(&index).unwrap_or_default().trim().to_owned())
                .collect(),
        );
    }
    Ok(rows)
}

fn number(text: &str) -> Option<Decimal> {
    text.parse()
        .ok()
        .or_else(|| Decimal::from_scientific(text).ok())
        .map(|value: Decimal| value.normalize())
}

fn named<'a>(header: &[String], row: &'a [String], names: &[&str]) -> Option<&'a str> {
    let index = header
        .iter()
        .position(|cell| names.contains(&cell.as_str()))?;
    row.get(index)
        .map(String::as_str)
        .filter(|text| !text.is_empty() && *text != "-")
}

fn ticker_matches(etf: &Symbol, rows: &[Vec<String>]) -> Result<(), SourceError> {
    let listed = rows
        .iter()
        .find(|row| row.first().is_some_and(|cell| cell == "Ticker Symbol:"))
        .and_then(|row| row.get(1))
        .ok_or_else(|| decode("no ticker symbol"))?;
    if Symbol::new(listed) == *etf {
        Ok(())
    } else {
        Err(decode(format!("the file is for {listed}, not {etf}")))
    }
}

pub fn normalize_holdings(
    etf: &Symbol,
    rows: &[Vec<String>],
) -> Result<EtfHoldingsFile, SourceError> {
    ticker_matches(etf, rows)?;
    let as_of = rows
        .iter()
        .find(|row| row.first().is_some_and(|cell| cell == "Holdings:"))
        .and_then(|row| row.get(1))
        .and_then(|text| text.strip_prefix("As of "))
        .and_then(|day| NaiveDate::parse_from_str(day, DATE_FORMAT).ok())
        .ok_or_else(|| decode("no holdings date"))?;
    let start = rows
        .iter()
        .position(|row| row.first().is_some_and(|cell| cell == "Name"))
        .ok_or_else(|| decode("no holdings header"))?;
    let header = &rows[start];
    let mut holdings = Vec::new();
    for row in &rows[start + 1..] {
        let (Some(name), Some(weight)) = (
            named(header, row, &["Name"]),
            named(header, row, &["Weight"]).and_then(number),
        ) else {
            break;
        };
        holdings.push(EtfHoldingRecord {
            name: name.to_owned(),
            ticker: named(header, row, &["Ticker"]).map(Symbol::new),
            identifier: named(header, row, &["Identifier"]).map(str::to_owned),
            weight,
            quantity: named(header, row, &["Shares Held", "Par Value"]).and_then(number),
        });
    }
    if holdings.is_empty() {
        return Err(decode(format!("{etf} lists no holdings")));
    }
    Ok(EtfHoldingsFile {
        etf: etf.clone(),
        as_of,
        holdings,
    })
}

pub fn normalize_shares(
    etf: &Symbol,
    rows: &[Vec<String>],
) -> Result<Vec<EtfSharesRecord>, SourceError> {
    ticker_matches(etf, rows)?;
    let start = rows
        .iter()
        .position(|row| row.first().is_some_and(|cell| cell == "Date"))
        .ok_or_else(|| decode("no NAV history header"))?;
    let header = &rows[start];
    let mut shares = Vec::new();
    for row in &rows[start + 1..] {
        let Some(date) = named(header, row, &["Date"])
            .and_then(|day| NaiveDate::parse_from_str(day, DATE_FORMAT).ok())
        else {
            break;
        };
        let value = |name: &str| named(header, row, &[name]).and_then(number);
        if let (Some(nav), Some(shares_outstanding), Some(net_assets)) = (
            value("NAV"),
            value("Shares Outstanding"),
            value("Total Net Assets"),
        ) {
            shares.push(EtfSharesRecord {
                date,
                nav,
                shares_outstanding,
                net_assets,
            });
        }
    }
    if shares.is_empty() {
        return Err(decode(format!("{etf} lists no NAV history")));
    }
    Ok(shares)
}
