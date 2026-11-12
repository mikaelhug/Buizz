use std::time::Duration;

use buizz_core::market::{BorrowAvailability, Symbol};
use buizz_core::time::Timestamp;
use chrono::{NaiveDate, NaiveTime};
use rust_decimal::Decimal;
use suppaftp::tokio::AsyncFtpStream;
use tokio::io::AsyncReadExt;

use crate::http::SourceError;

pub const ADDRESS: &str = "ftp2.interactivebrokers.com:21";
pub const USER: &str = "shortstock";
pub const US_FILE: &str = "usa.txt";
const TIMEOUT: Duration = Duration::from_secs(120);
const COLUMNS: [&str; 8] = [
    "#SYM",
    "CUR",
    "NAME",
    "CON",
    "ISIN",
    "REBATERATE",
    "FEERATE",
    "AVAILABLE",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IbkrConfig {
    pub address: String,
}

#[derive(Clone)]
pub struct IbkrClient {
    address: String,
}

impl IbkrClient {
    pub fn new(config: IbkrConfig) -> Self {
        Self {
            address: config.address,
        }
    }

    pub async fn us_borrow(&self) -> Result<BorrowFile, SourceError> {
        let payload = tokio::time::timeout(TIMEOUT, self.download(US_FILE))
            .await
            .map_err(|_| SourceError::Ftp(format!("{US_FILE} took longer than {TIMEOUT:?}")))??;
        normalize_borrow(&payload)
    }

    async fn download(&self, file: &str) -> Result<Vec<u8>, SourceError> {
        let ftp = |error: suppaftp::FtpError| SourceError::Ftp(error.to_string());
        let mut stream = AsyncFtpStream::connect(self.address.as_str())
            .await
            .map_err(ftp)?;
        stream.login(USER, "").await.map_err(ftp)?;
        let mut download = stream.retr_as_stream(file).await.map_err(ftp)?;
        let mut payload = Vec::new();
        download
            .read_to_end(&mut payload)
            .await
            .map_err(|error| SourceError::Ftp(error.to_string()))?;
        download.finish().await.map_err(ftp)?;
        stream.quit().await.map_err(ftp)?;
        Ok(payload)
    }
}

fn decode(message: impl Into<String>) -> SourceError {
    SourceError::Decode(format!("IBKR short stock file: {}", message.into()))
}

fn rate(text: &str) -> Result<Option<Decimal>, SourceError> {
    match text {
        "NA" | "" => Ok(None),
        number => number
            .parse()
            .map(Some)
            .map_err(|_| decode(format!("rate {number:?}"))),
    }
}

pub struct BorrowFile {
    pub records: Vec<BorrowAvailability>,
    pub malformed_rows: usize,
}

pub fn normalize_borrow(payload: &[u8]) -> Result<BorrowFile, SourceError> {
    let text = String::from_utf8_lossy(payload);
    let mut lines = text.lines().map(str::trim_end);
    let header = lines.next().ok_or_else(|| decode("empty"))?;
    let at = match header.split('|').collect::<Vec<_>>().as_slice() {
        ["#BOF", day, time, ..] => NaiveDate::parse_from_str(day, "%Y.%m.%d")
            .ok()
            .zip(NaiveTime::parse_from_str(time, "%H:%M:%S").ok())
            .and_then(|(day, time)| Timestamp::at_exchange_time(day, time)),
        _ => None,
    }
    .ok_or_else(|| decode(format!("first line {header:?}")))?;
    let columns = lines.next().unwrap_or_default();
    if !columns.split('|').take(COLUMNS.len()).eq(COLUMNS) {
        return Err(decode(format!("columns {columns:?}")));
    }
    let mut file = BorrowFile {
        records: Vec::new(),
        malformed_rows: 0,
    };
    for (rows, line) in lines.enumerate() {
        if let Some(count) = line.strip_prefix("#EOF|") {
            if count.parse::<usize>() != Ok(rows) {
                return Err(decode(format!("{rows} rows but the trailer says {count}")));
            }
            if file.malformed_rows * 2 > rows {
                return Err(decode(format!(
                    "{} of {rows} rows are malformed",
                    file.malformed_rows
                )));
            }
            return Ok(file);
        }
        match record(line, at) {
            Ok(Some(record)) => file.records.push(record),
            Ok(None) => {}
            Err(_) => file.malformed_rows += 1,
        }
    }
    Err(decode("no #EOF trailer, the download is incomplete"))
}

fn record(line: &str, at: Timestamp) -> Result<Option<BorrowAvailability>, SourceError> {
    let fields: Vec<&str> = line.split('|').collect();
    let [symbol, currency, _, con_id, _, rebate, fee, available, ..] = fields.as_slice() else {
        return Err(decode(format!("row {line:?}")));
    };
    let symbol = symbol.replace(' ', ".");
    if *currency != "USD"
        || !symbol
            .chars()
            .all(|letter| letter.is_ascii_alphanumeric() || letter == '.')
    {
        return Ok(None);
    }
    let (available, available_or_more) = match available.strip_prefix('>') {
        Some(floor) => (floor, true),
        None => (*available, false),
    };
    Ok(Some(BorrowAvailability {
        symbol: Symbol::new(&symbol),
        at,
        con_id: con_id
            .parse()
            .map_err(|_| decode(format!("contract id {con_id:?}")))?,
        rebate_rate: rate(rebate)?,
        fee_rate: rate(fee)?,
        available: available
            .parse()
            .map_err(|_| decode(format!("available {available:?}")))?,
        available_or_more,
    }))
}
