use std::collections::{BTreeMap, HashSet};

use buizz_core::market::{DataSource, Dataset, MacroObservation, MacroSeries, Symbol};
use buizz_core::ops::JobOutcome;
use buizz_core::time::Timestamp;
use buizz_sources::cboe::{FIRST_DAILY_PUT_CALL, LAST_ARCHIVED_PUT_CALL};
use chrono::{NaiveDate, TimeDelta};

use crate::fetch::{Batch, FetchKey, Work, read};
use crate::{Ingest, IngestError, schedule};

const PUT_CALL_DAYS_PER_RUN: usize = 250;
const PUT_CALL_LATE_AFTER: TimeDelta = TimeDelta::days(3);

pub(crate) fn series_key(series: MacroSeries) -> Symbol {
    Symbol::new(&format!("{series:?}"))
}

impl Ingest {
    pub(crate) async fn series_unread(&self, job: &str) -> Result<bool, IngestError> {
        let (source, series): (DataSource, &[MacroSeries]) = match job {
            schedule::MACRO => (DataSource::Fred, &MacroSeries::FRED),
            schedule::VIX => (DataSource::Cboe, &MacroSeries::CBOE),
            schedule::PUT_CALL => (DataSource::Cboe, &MacroSeries::PUT_CALL),
            _ => return Ok(false),
        };
        for series in series {
            let fetch = self
                .latest_fetch(source, Dataset::Macro, &series_key(*series))
                .await?;
            if !fetch.is_some_and(|fetch| read(&fetch)) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) async fn macro_series(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(fred) = &self.sources.fred else {
            return Ok(Work::unconfigured("FRED"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Fred).await? {
            return Ok(work);
        }
        let mut batch = Batch::default();
        for series in MacroSeries::FRED {
            let symbol = series_key(series);
            let key = FetchKey::new(DataSource::Fred, Dataset::Macro, Some(&symbol), None);
            let stored: HashSet<MacroObservation> = self
                .market
                .macro_observations(series)
                .await?
                .into_iter()
                .collect();
            if let Some(last) = stored.iter().map(|observation| observation.published).max() {
                match fred.vintage_dates_since(series, last).await {
                    Ok(dates) if dates.iter().all(|date| *date <= last) => {
                        self.record_fetched(now, &key, 0).await?;
                        batch.success(0);
                        continue;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        self.record_failure(now, &key, &error).await?;
                        let label = series.fred_id().unwrap_or_default();
                        if batch.source_error(label, &error) {
                            break;
                        }
                        continue;
                    }
                }
            }
            match fred.vintages(series).await {
                Ok(vintages) => {
                    let fetched = vintages.len();
                    let changed: Vec<MacroObservation> = vintages
                        .into_iter()
                        .filter(|vintage| !stored.contains(vintage))
                        .collect();
                    let rows = if changed.is_empty() {
                        0
                    } else {
                        self.market
                            .upsert_macro_observations(DataSource::Fred, changed)
                            .await?
                            .rows
                    };
                    self.record_fetched(now, &key, fetched).await?;
                    batch.success(rows);
                }
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    let label = series.fred_id().unwrap_or_default();
                    if batch.source_error(label, &error) {
                        break;
                    }
                }
            }
        }
        Ok(batch.finish("macro"))
    }

    pub(crate) async fn vix(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(cboe) = &self.sources.cboe else {
            return Ok(Work::unconfigured("CBOE"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Cboe).await? {
            return Ok(work);
        }
        let today = now.exchange_date();
        let mut batch = Batch::default();
        for series in MacroSeries::CBOE {
            let symbol = series_key(series);
            let key = FetchKey::new(DataSource::Cboe, Dataset::Macro, Some(&symbol), None);
            let closes = match cboe.daily_closes(series).await {
                Ok(closes) => closes,
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    if batch.source_error(&format!("{series:?}"), &error) {
                        break;
                    }
                    continue;
                }
            };
            self.record_fetched(now, &key, closes.len()).await?;
            let latest: BTreeMap<NaiveDate, MacroObservation> = self
                .market
                .macro_observations(series)
                .await?
                .into_iter()
                .map(|observation| (observation.date, observation))
                .collect();
            let changed: Vec<MacroObservation> = closes
                .into_iter()
                .filter_map(|close| match latest.get(&close.date) {
                    None => Some(close),
                    Some(known) if known.value == close.value => None,
                    Some(_) => Some(MacroObservation {
                        published: today.max(close.date),
                        ..close
                    }),
                })
                .collect();
            let rows = if changed.is_empty() {
                0
            } else {
                self.market
                    .upsert_macro_observations(DataSource::Cboe, changed)
                    .await?
                    .rows
            };
            batch.success(rows);
        }
        Ok(batch.finish("vix"))
    }

    pub(crate) async fn put_call(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(cboe) = &self.sources.cboe else {
            return Ok(Work::unconfigured("CBOE"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Cboe).await? {
            return Ok(work);
        }
        let today = now.exchange_date();
        let mut batch = Batch::default();
        let mut latest_per_series = Vec::new();
        for series in MacroSeries::PUT_CALL {
            let stored = self.market.macro_observations(series).await?;
            let latest = stored.iter().map(|observation| observation.date).max();
            if latest.is_some_and(|date| date >= LAST_ARCHIVED_PUT_CALL) {
                latest_per_series.push(latest);
                continue;
            }
            let key = FetchKey::new(
                DataSource::Cboe,
                Dataset::Macro,
                Some(&series_key(series)),
                None,
            );
            match cboe.archived_put_call(series).await {
                Ok(ratios) => {
                    latest_per_series.push(latest.max(ratios.iter().map(|ratio| ratio.date).max()));
                    let known: HashSet<NaiveDate> =
                        stored.iter().map(|observation| observation.date).collect();
                    let fresh: Vec<MacroObservation> = ratios
                        .into_iter()
                        .filter(|ratio| !known.contains(&ratio.date))
                        .collect();
                    let rows = if fresh.is_empty() {
                        0
                    } else {
                        self.market
                            .upsert_macro_observations(DataSource::Cboe, fresh)
                            .await?
                            .rows
                    };
                    batch.success(rows);
                }
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    batch.source_error(&format!("{series:?} archive"), &error);
                    return Ok(batch.finish("put/call"));
                }
            }
        }
        let from = latest_per_series
            .into_iter()
            .min()
            .flatten()
            .map_or(FIRST_DAILY_PUT_CALL, |last| last + TimeDelta::days(1))
            .max(FIRST_DAILY_PUT_CALL);
        let closed: Vec<NaiveDate> = self
            .market
            .calendar(from, today)
            .await?
            .sessions()
            .iter()
            .filter(|session| session.close <= now)
            .map(|session| session.date)
            .collect();
        let mut current = true;
        let mut unpublished = None;
        for (index, date) in closed.iter().copied().enumerate() {
            if self.stopping() {
                current = false;
                break;
            }
            if index == PUT_CALL_DAYS_PER_RUN {
                batch.more_remains(format!("days from {date}"));
                current = false;
                break;
            }
            let key = FetchKey::new(DataSource::Cboe, Dataset::Macro, None, Some((date, date)));
            match cboe.daily_put_call(date).await {
                Ok(Some(ratios)) => {
                    let summary = self
                        .market
                        .upsert_macro_observations(DataSource::Cboe, ratios)
                        .await?;
                    batch.success(summary.rows);
                }
                Ok(None) if date + PUT_CALL_LATE_AFTER < today => {
                    batch.failure(&date.to_string(), &"CBOE published no statistics");
                }
                Ok(None) => {
                    unpublished = Some(date);
                    break;
                }
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    batch.source_error(&date.to_string(), &error);
                    current = false;
                    break;
                }
            }
        }
        if current {
            for series in MacroSeries::PUT_CALL {
                let key = FetchKey::new(
                    DataSource::Cboe,
                    Dataset::Macro,
                    Some(&series_key(series)),
                    None,
                );
                self.record_fetched(now, &key, closed.len()).await?;
            }
        }
        let work = batch.finish("put/call");
        Ok(match unpublished {
            Some(date) if work.outcome == JobOutcome::Noop => {
                Work::waiting(format!("put/call: {date} is not published yet"))
            }
            _ => work,
        })
    }
}
