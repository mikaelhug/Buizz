use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use buizz_core::market::{Company, Symbol};
use buizz_core::time::Timestamp;
use chrono::{DateTime, NaiveDate};
use reqwest::header::LAST_MODIFIED;
use tokio::io::AsyncWriteExt;
use zip::ZipArchive;
use zip::result::ZipError;

use super::filings::{Filings, normalize_filings_page, normalize_submissions};
use crate::http::SourceError;

pub const SUBMISSIONS_ARCHIVE_PATH: &str = "/Archives/edgar/daily-index/bulkdata/submissions.zip";
pub(super) const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60 * 60);

#[derive(Clone)]
pub struct SubmissionsArchive {
    pub filed_through: NaiveDate,
    entries: Arc<Mutex<ZipArchive<File>>>,
}

pub(super) fn filed_through(response: &reqwest::Response) -> Result<NaiveDate, SourceError> {
    let header = response
        .headers()
        .get(LAST_MODIFIED)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            SourceError::Decode("submissions archive without Last-Modified".to_owned())
        })?;
    DateTime::parse_from_rfc2822(header)
        .ok()
        .and_then(|time| Timestamp::from_datetime(time.to_utc()))
        .and_then(|published| published.exchange_date().pred_opt())
        .ok_or_else(|| SourceError::Decode(format!("submissions archive Last-Modified {header:?}")))
}

fn stream_error(error: &impl std::fmt::Display) -> SourceError {
    SourceError::Stream(format!("submissions archive: {error}"))
}

impl SubmissionsArchive {
    pub(super) async fn download(
        mut response: reqwest::Response,
        filed_through: NaiveDate,
        scratch: &Path,
    ) -> Result<Self, SourceError> {
        let file = tempfile::tempfile_in(scratch).map_err(|error| stream_error(&error))?;
        let mut writer = tokio::fs::File::from_std(file);
        while let Some(chunk) = response.chunk().await? {
            writer
                .write_all(&chunk)
                .await
                .map_err(|error| stream_error(&error))?;
        }
        writer.flush().await.map_err(|error| stream_error(&error))?;
        let file = writer.into_std().await;
        let entries = tokio::task::spawn_blocking(move || ZipArchive::new(file))
            .await
            .map_err(|error| stream_error(&error))?
            .map_err(|error| stream_error(&error))?;
        Ok(Self {
            filed_through,
            entries: Arc::new(Mutex::new(entries)),
        })
    }

    pub async fn company_filings(
        &self,
        symbol: &Symbol,
        cik: u32,
        since: Option<NaiveDate>,
    ) -> Result<Option<(Company, Filings)>, SourceError> {
        let entries = self.entries.clone();
        let symbol = symbol.clone();
        tokio::task::spawn_blocking(move || {
            let mut entries = entries.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(recent) = entry(&mut entries, &format!("CIK{cik:010}.json"))? else {
                return Ok(None);
            };
            let submissions = normalize_submissions(&symbol, &recent)?;
            let mut filings = submissions.filings.clone();
            for page in submissions.pages_since(since) {
                let payload = entry(&mut entries, &page.name)?.ok_or_else(|| {
                    SourceError::Decode(format!("submissions archive lacks {}", page.name))
                })?;
                filings.extend(normalize_filings_page(&symbol, cik, &payload)?);
            }
            Ok(Some((submissions.company, filings)))
        })
        .await
        .map_err(|error| stream_error(&error))?
    }
}

fn entry(entries: &mut ZipArchive<File>, name: &str) -> Result<Option<Vec<u8>>, SourceError> {
    let mut file = match entries.by_name(name) {
        Ok(file) => file,
        Err(ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(stream_error(&error)),
    };
    let mut payload = Vec::new();
    file.read_to_end(&mut payload)
        .map_err(|error| stream_error(&error))?;
    Ok(Some(payload))
}
