use std::fmt::Display;

use buizz_core::market::{DataSource, Dataset, Symbol};
use buizz_core::ops::{FetchStatus, JobOutcome, SourceFetch};
use buizz_core::time::Timestamp;
use buizz_sources::http::SourceError;
use chrono::{NaiveDate, TimeDelta};

use crate::{Ingest, IngestError};

pub(crate) const EDGAR_IN_FLIGHT: usize = 8;

const NOT_FOUND: u16 = 404;
const TOO_MANY_REQUESTS: u16 = 429;
const MAX_LISTED_FAILURES: usize = 3;

pub(crate) struct Work {
    pub outcome: JobOutcome,
    pub rows: u64,
    pub message: String,
}

impl Work {
    pub fn ok(rows: u64, message: impl Into<String>) -> Self {
        Self::of(JobOutcome::Ok, rows, message)
    }

    pub fn partial(rows: u64, message: impl Into<String>) -> Self {
        Self::of(JobOutcome::Partial, rows, message)
    }

    pub fn noop(message: impl Into<String>) -> Self {
        Self::of(JobOutcome::Noop, 0, message)
    }

    pub fn waiting(message: impl Into<String>) -> Self {
        Self::of(JobOutcome::Waiting, 0, message)
    }

    pub fn backoff(rows: u64, message: impl Into<String>) -> Self {
        Self::of(JobOutcome::Backoff, rows, message)
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::of(JobOutcome::Failed, 0, message)
    }

    pub fn unconfigured(what: &str) -> Self {
        Self::noop(format!("{what} is not configured"))
    }

    pub fn from_failure(error: &SourceError) -> Self {
        match error {
            SourceError::RateLimited { .. } => Self::backoff(0, error.to_string()),
            _ => Self::failed(error.to_string()),
        }
    }

    fn of(outcome: JobOutcome, rows: u64, message: impl Into<String>) -> Self {
        Self {
            outcome,
            rows,
            message: message.into(),
        }
    }
}

#[derive(Default)]
pub(crate) struct Batch {
    attempted: usize,
    rows: u64,
    failures: Vec<String>,
    rate_limited: Option<String>,
    remaining: Option<String>,
}

impl Batch {
    pub fn more_remains(&mut self, what: impl Into<String>) {
        self.remaining = Some(what.into());
    }

    pub fn success(&mut self, rows: u64) {
        self.attempted += 1;
        self.rows += rows;
    }

    pub fn failure(&mut self, label: &str, error: &dyn Display) {
        self.attempted += 1;
        self.failures.push(format!("{label}: {error}"));
    }

    pub fn source_error(&mut self, label: &str, error: &SourceError) -> bool {
        if matches!(error, SourceError::RateLimited { .. }) {
            self.attempted += 1;
            self.rate_limited = Some(error.to_string());
            true
        } else {
            self.failure(label, error);
            false
        }
    }

    pub fn finish(self, what: &str) -> Work {
        if let Some(limit) = self.rate_limited {
            return Work::backoff(
                self.rows,
                format!("{what}: {limit}; {} rows before the limit", self.rows),
            );
        }
        if self.attempted == 0 {
            return Work::noop(format!("{what}: nothing to fetch"));
        }
        let failed = self.failures.len();
        let listed = self
            .failures
            .iter()
            .take(MAX_LISTED_FAILURES)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        if failed == self.attempted {
            return Work::failed(format!("{what}: all {failed} failed: {listed}"));
        }
        let note = if failed == 0 {
            String::new()
        } else {
            format!(", {failed} failed: {listed}")
        };
        if let Some(remaining) = self.remaining {
            Work::partial(
                self.rows,
                format!("{what}: {} rows, {remaining} left{note}", self.rows),
            )
        } else if self.rows > 0 {
            Work::ok(self.rows, format!("{what}: {} rows{note}", self.rows))
        } else {
            Work::noop(format!("{what}: nothing new{note}"))
        }
    }
}

pub(crate) struct FetchKey {
    source: DataSource,
    dataset: Dataset,
    symbol: Option<Symbol>,
    range: Option<(NaiveDate, NaiveDate)>,
}

impl FetchKey {
    pub fn new(
        source: DataSource,
        dataset: Dataset,
        symbol: Option<&Symbol>,
        range: Option<(NaiveDate, NaiveDate)>,
    ) -> Self {
        Self {
            source,
            dataset,
            symbol: symbol.cloned(),
            range,
        }
    }
}

struct Attempt {
    status: FetchStatus,
    http_status: Option<u16>,
    rows: u64,
    retry_after: Option<Timestamp>,
    message: Option<String>,
}

impl Attempt {
    fn of(status: FetchStatus, message: Option<String>) -> Self {
        Self {
            status,
            http_status: None,
            rows: 0,
            retry_after: None,
            message,
        }
    }
}

pub(crate) fn read(fetch: &SourceFetch) -> bool {
    matches!(fetch.status, FetchStatus::Ok | FetchStatus::Empty)
}

pub(crate) fn waiting(fetch: &SourceFetch, now: Timestamp) -> bool {
    fetch.retry_after.is_some_and(|until| until > now)
}

impl Ingest {
    pub(crate) async fn latest_fetch(
        &self,
        source: DataSource,
        dataset: Dataset,
        symbol: &Symbol,
    ) -> Result<Option<SourceFetch>, IngestError> {
        Ok(self
            .state
            .recent_fetches(source, dataset, Some(symbol.clone()), 1)
            .await?
            .pop())
    }

    async fn record(
        &self,
        now: Timestamp,
        key: &FetchKey,
        attempt: Attempt,
    ) -> Result<(), IngestError> {
        let symbol = match attempt.status {
            FetchStatus::RateLimited => None,
            _ => key.symbol.clone(),
        };
        self.state
            .record_fetch(SourceFetch {
                source: key.source,
                dataset: key.dataset,
                symbol,
                requested_from: key.range.map(|(from, _)| from),
                requested_to: key.range.map(|(_, to)| to),
                at: now,
                status: attempt.status,
                http_status: attempt.http_status,
                rows: attempt.rows,
                retry_after: attempt.retry_after,
                message: attempt.message,
            })
            .await?;
        Ok(())
    }

    pub(crate) async fn record_fetched(
        &self,
        now: Timestamp,
        key: &FetchKey,
        rows: usize,
    ) -> Result<(), IngestError> {
        let status = if rows == 0 {
            FetchStatus::Empty
        } else {
            FetchStatus::Ok
        };
        let attempt = Attempt {
            rows: rows as u64,
            ..Attempt::of(status, None)
        };
        self.record(now, key, attempt).await
    }

    pub(crate) async fn record_rejection(
        &self,
        now: Timestamp,
        key: &FetchKey,
        message: &str,
    ) -> Result<(), IngestError> {
        let attempt = Attempt::of(FetchStatus::Rejected, Some(message.to_owned()));
        self.record(now, key, attempt).await
    }

    pub(crate) async fn record_failure(
        &self,
        now: Timestamp,
        key: &FetchKey,
        error: &SourceError,
    ) -> Result<(), IngestError> {
        let message = Some(error.to_string());
        let attempt = match error {
            SourceError::RateLimited { retry_after } => Attempt {
                http_status: Some(TOO_MANY_REQUESTS),
                retry_after: retry_after
                    .and_then(|wait| TimeDelta::from_std(wait).ok())
                    .map(|wait| now + wait),
                ..Attempt::of(FetchStatus::RateLimited, message)
            },
            SourceError::Status { status, .. } => Attempt {
                http_status: Some(*status),
                ..Attempt::of(
                    if *status == NOT_FOUND {
                        FetchStatus::Rejected
                    } else {
                        FetchStatus::Failed
                    },
                    message,
                )
            },
            _ => Attempt::of(FetchStatus::Failed, message),
        };
        self.record(now, key, attempt).await
    }

    pub(crate) async fn source_failure(
        &self,
        now: Timestamp,
        key: &FetchKey,
        error: &SourceError,
    ) -> Result<Work, IngestError> {
        self.record_failure(now, key, error).await?;
        Ok(Work::from_failure(error))
    }

    pub(crate) async fn rate_limited_until(
        &self,
        now: Timestamp,
        source: DataSource,
    ) -> Result<Option<Timestamp>, IngestError> {
        Ok(self
            .state
            .rate_limited_until(source)
            .await?
            .filter(|until| *until > now))
    }

    pub(crate) async fn backoff_work(
        &self,
        now: Timestamp,
        source: DataSource,
    ) -> Result<Option<Work>, IngestError> {
        Ok(self
            .rate_limited_until(now, source)
            .await?
            .map(|until| Work::backoff(0, format!("rate limited until {until}"))))
    }
}
