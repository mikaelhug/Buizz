use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::Duration;

use buizz_core::market::{ShortInterestRecord, ShortVolumeRecord, Symbol};
use chrono::{Datelike, NaiveDate};
use regex::Regex;
use rust_decimal::Decimal;
use serde_json::json;

use crate::http::{HttpClient, RateLimit, SourceError};

pub const API_URL: &str = "https://api.finra.org";
pub const CDN_URL: &str = "https://cdn.finra.org";
pub const SITE_URL: &str = "https://www.finra.org";
pub const API_URL_ENV: &str = "FINRA_API_URL";
pub const CDN_URL_ENV: &str = "FINRA_CDN_URL";
pub const SITE_URL_ENV: &str = "FINRA_URL";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 4,
    per: Duration::from_secs(1),
    cooldown: Duration::from_secs(60),
};
pub const FIRST_SETTLEMENT: NaiveDate = NaiveDate::from_ymd_opt(2017, 12, 29).expect("valid date");
pub const FIRST_SHORT_VOLUME: NaiveDate = NaiveDate::from_ymd_opt(2018, 8, 1).expect("valid date");
pub const SHORT_INTEREST_PATH: &str = "/data/group/otcMarket/name/consolidatedShortInterest";
pub const SCHEDULE_PATH: &str = "/filing-reporting/regulatory-filing-systems/short-interest";
const PAGE_ROWS: usize = 5000;
const NOT_PUBLISHED: [u16; 2] = [403, 404];
const LONGEST_PUBLICATION_LAG_DAYS: i64 = 40;
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

static SCHEDULE_HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\d{4})(?:&nbsp;|\s)*Short Interest Reporting Dates\s*</h\d>")
        .expect("valid regex")
});
static CELL_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"({})\s+(\d{{1,2}})(?:,\s*(\d{{4}}))?",
        MONTHS.join("|")
    ))
    .expect("valid regex")
});
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").expect("valid regex"));

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinraConfig {
    pub api_url: String,
    pub cdn_url: String,
    pub site_url: String,
}

#[derive(Clone)]
pub struct FinraClient {
    http: HttpClient,
    config: FinraConfig,
}

pub struct FinraFile<T> {
    pub records: Vec<T>,
    pub malformed_rows: usize,
}

impl FinraClient {
    pub fn new(config: FinraConfig) -> Result<Self, SourceError> {
        let trim = |url: &str| url.trim_end_matches('/').to_owned();
        Ok(Self {
            http: HttpClient::new("buizz", RATE_LIMIT)?,
            config: FinraConfig {
                api_url: trim(&config.api_url),
                cdn_url: trim(&config.cdn_url),
                site_url: trim(&config.site_url),
            },
        })
    }

    pub async fn schedule(&self) -> Result<BTreeMap<NaiveDate, NaiveDate>, SourceError> {
        let url = format!("{}{SCHEDULE_PATH}", self.config.site_url);
        let page = self.http.fetch(self.http.client().get(url)).await?;
        parse_schedule(&String::from_utf8_lossy(&page))
    }

    pub async fn short_interest(
        &self,
        settlement: NaiveDate,
        published: NaiveDate,
    ) -> Result<FinraFile<ShortInterestRecord>, SourceError> {
        let url = format!("{}{SHORT_INTEREST_PATH}", self.config.api_url);
        let mut file = FinraFile {
            records: Vec::new(),
            malformed_rows: 0,
        };
        for offset in (0_usize..).step_by(PAGE_ROWS) {
            let request = self
                .http
                .client()
                .post(&url)
                .header(reqwest::header::ACCEPT, "text/plain")
                .json(&json!({
                    "limit": PAGE_ROWS,
                    "offset": offset,
                    "sortFields": ["symbolCode"],
                    "compareFilters": [{
                        "compareType": "EQUAL",
                        "fieldName": "settlementDate",
                        "fieldValue": settlement.to_string(),
                    }],
                }));
            let page = self.http.fetch(request).await?;
            let rows = parse_short_interest(&page, settlement, published, &mut file)?;
            if rows < PAGE_ROWS {
                return Ok(file);
            }
        }
        unreachable!("the offsets never run out")
    }

    pub async fn short_volume(
        &self,
        date: NaiveDate,
    ) -> Result<Option<FinraFile<ShortVolumeRecord>>, SourceError> {
        let url = format!(
            "{}/equity/regsho/daily/CNMSshvol{}.txt",
            self.config.cdn_url,
            date.format("%Y%m%d")
        );
        match self.http.fetch(self.http.client().get(url)).await {
            Ok(file) => parse_short_volume(&String::from_utf8_lossy(&file), date).map(Some),
            Err(SourceError::Status { status, .. }) if NOT_PUBLISHED.contains(&status) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

fn decode(message: impl Into<String>) -> SourceError {
    SourceError::Decode(format!("FINRA: {}", message.into()))
}

fn parse_short_interest(
    page: &[u8],
    settlement: NaiveDate,
    published: NaiveDate,
    file: &mut FinraFile<ShortInterestRecord>,
) -> Result<usize, SourceError> {
    if page.iter().all(u8::is_ascii_whitespace) {
        return Ok(0);
    }
    let mut reader = csv::Reader::from_reader(page);
    let headers = reader
        .headers()
        .map_err(|error| decode(error.to_string()))?
        .clone();
    let column = |name: &str| {
        headers
            .iter()
            .position(|header| header == name)
            .ok_or_else(|| decode(format!("short interest without a {name:?} column")))
    };
    let [symbol, shares, previous, volume, days, split, revised, date] = [
        "symbolCode",
        "currentShortPositionQuantity",
        "previousShortPositionQuantity",
        "averageDailyVolumeQuantity",
        "daysToCoverQuantity",
        "stockSplitFlag",
        "revisionFlag",
        "settlementDate",
    ]
    .map(column);
    let (symbol, shares, previous, volume, days, split, revised, date) = (
        symbol?, shares?, previous?, volume?, days?, split?, revised?, date?,
    );
    let rows: Vec<_> = reader.records().collect();
    for row in &rows {
        let Ok(row) = row else {
            file.malformed_rows += 1;
            continue;
        };
        let field = |index: usize| row.get(index).unwrap_or_default().trim();
        if field(date) != settlement.to_string() {
            return Err(decode(format!(
                "asked for settlement {settlement}, got {:?}",
                field(date)
            )));
        }
        let record = (|| {
            Some(ShortInterestRecord {
                symbol: Some(field(symbol))
                    .filter(|text| !text.is_empty())
                    .map(Symbol::new)?,
                settlement,
                shares: field(shares).parse().ok()?,
                previous_shares: field(previous).parse().ok()?,
                average_daily_volume: field(volume).parse().ok()?,
                days_to_cover: field(days).parse().ok()?,
                split: !field(split).is_empty(),
                revised: !field(revised).is_empty(),
                published,
            })
        })();
        match record {
            Some(record) => file.records.push(record),
            None => file.malformed_rows += 1,
        }
    }
    Ok(rows.len())
}

pub fn parse_short_volume(
    text: &str,
    date: NaiveDate,
) -> Result<FinraFile<ShortVolumeRecord>, SourceError> {
    let mut lines = text.lines().map(str::trim_end);
    let header: Vec<&str> = lines.next().unwrap_or_default().split('|').collect();
    let column = |name: &str| {
        header
            .iter()
            .position(|column| *column == name)
            .ok_or_else(|| decode(format!("short volume without a {name:?} column")))
    };
    let (day, symbol, short, exempt, total) = (
        column("Date")?,
        column("Symbol")?,
        column("ShortVolume")?,
        column("ShortExemptVolume")?,
        column("TotalVolume")?,
    );
    let stamp = date.format("%Y%m%d").to_string();
    let mut file = FinraFile {
        records: Vec::new(),
        malformed_rows: 0,
    };
    for (rows, line) in lines.filter(|line| !line.is_empty()).enumerate() {
        if line.bytes().all(|byte| byte.is_ascii_digit()) {
            return match line.parse::<usize>() {
                Ok(count) if count == rows => Ok(file),
                _ => Err(decode(format!("{rows} rows but the trailer says {line}"))),
            };
        }
        let fields: Vec<&str> = line.split('|').collect();
        let field = |index: usize| fields.get(index).copied().unwrap_or_default().trim();
        if field(day) != stamp {
            return Err(decode(format!(
                "asked for short volume of {date}, got {:?}",
                field(day)
            )));
        }
        let volume = |index: usize| field(index).parse::<Decimal>().ok();
        let record = (|| {
            Some(ShortVolumeRecord {
                symbol: Some(field(symbol))
                    .filter(|text| !text.is_empty())
                    .map(Symbol::new)?,
                date,
                short_volume: volume(short)?,
                short_exempt_volume: volume(exempt)?,
                total_volume: volume(total)?,
            })
        })();
        match record {
            Some(record) => file.records.push(record),
            None => file.malformed_rows += 1,
        }
    }
    Err(decode(format!(
        "short volume of {date} has no row count trailer, the download is incomplete"
    )))
}

pub fn parse_schedule(page: &str) -> Result<BTreeMap<NaiveDate, NaiveDate>, SourceError> {
    let mut schedule = BTreeMap::new();
    for heading in SCHEDULE_HEADING.captures_iter(page) {
        let year: i32 = heading[1]
            .parse()
            .map_err(|_| decode(format!("schedule year {:?}", &heading[1])))?;
        let rest = &page[heading.get(0).map_or(0, |whole| whole.end())..];
        let table = &rest[..rest.find("</table>").unwrap_or(rest.len())];
        for row in table.split("<tr").skip(1) {
            let cells: Vec<String> = row
                .split("<td")
                .skip(1)
                .map(|cell| {
                    TAG.replace_all(&format!("<{cell}"), " ")
                        .replace("&nbsp;", " ")
                })
                .collect();
            let [settlement, _, publication] = cells.as_slice() else {
                continue;
            };
            let settlement = cell_date(settlement, year)?;
            let mut published = cell_date(publication, settlement.year())?;
            if published < settlement {
                published = published
                    .with_year(published.year() + 1)
                    .ok_or_else(|| decode(format!("publication date after {settlement}")))?;
            }
            if (published - settlement).num_days() > LONGEST_PUBLICATION_LAG_DAYS {
                return Err(decode(format!(
                    "settlement {settlement} published only on {published}"
                )));
            }
            schedule.insert(settlement, published);
        }
    }
    Ok(schedule)
}

fn cell_date(cell: &str, year: i32) -> Result<NaiveDate, SourceError> {
    let captures = CELL_DATE
        .captures(cell)
        .ok_or_else(|| decode(format!("schedule cell {:?} has no date", cell.trim())))?;
    let month = MONTHS
        .iter()
        .position(|month| *month == &captures[1])
        .and_then(|index| u32::try_from(index + 1).ok());
    let year = captures
        .get(3)
        .and_then(|explicit| explicit.as_str().parse().ok())
        .unwrap_or(year);
    month
        .zip(captures[2].parse().ok())
        .and_then(|(month, day)| NaiveDate::from_ymd_opt(year, month, day))
        .ok_or_else(|| decode(format!("schedule cell {:?}", cell.trim())))
}
