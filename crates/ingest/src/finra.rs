use std::collections::{BTreeMap, BTreeSet};

use buizz_core::market::{Calendar, DataSource, Dataset, ShortInterestRecord, Symbol};
use buizz_core::ops::JobOutcome;
use buizz_core::time::Timestamp;
use buizz_core::visibility::{CALENDAR_HORIZON, short_interest_published};
use buizz_sources::finra::{FIRST_SETTLEMENT, FIRST_SHORT_VOLUME, FinraFile};
use chrono::{Datelike, NaiveDate, TimeDelta};

use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError};

const MID_MONTH_DAY: u32 = 15;
const REREAD_SETTLEMENTS: usize = 4;
const SETTLEMENTS_PER_RUN: usize = 40;
const SHORT_VOLUME_FILES_PER_RUN: usize = 100;
const SHORT_INTEREST_LATE_AFTER: TimeDelta = TimeDelta::days(7);
const SHORT_VOLUME_LATE_AFTER: TimeDelta = TimeDelta::days(3);

pub(crate) fn settlement_dates(calendar: &Calendar, through: NaiveDate) -> BTreeSet<NaiveDate> {
    let mut months: BTreeMap<(i32, u32), (Option<NaiveDate>, NaiveDate)> = BTreeMap::new();
    for session in calendar.sessions() {
        let date = session.date;
        let month = months
            .entry((date.year(), date.month()))
            .or_insert((None, date));
        if date.day() <= MID_MONTH_DAY {
            month.0 = Some(date);
        }
        month.1 = date;
    }
    months
        .into_values()
        .flat_map(|(mid, end)| mid.into_iter().chain([end]))
        .filter(|date| (FIRST_SETTLEMENT..=through).contains(date))
        .collect()
}

fn same_position(a: &ShortInterestRecord, b: &ShortInterestRecord) -> bool {
    ShortInterestRecord {
        published: b.published,
        ..a.clone()
    } == *b
}

impl Ingest {
    pub(crate) async fn short_interest(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(finra) = &self.sources.finra else {
            return Ok(Work::unconfigured("FINRA"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Finra).await? {
            return Ok(work);
        }
        let today = now.exchange_date();
        let key = FetchKey::new(DataSource::Finra, Dataset::ShortInterest, None, None);
        let mut batch = Batch::default();
        let schedule = match finra.schedule().await {
            Ok(schedule) => schedule,
            Err(error) => {
                self.record_failure(now, &key, &error).await?;
                if batch.source_error("schedule", &error) {
                    return Ok(batch.finish("short interest"));
                }
                BTreeMap::new()
            }
        };
        let calendar = self
            .market
            .calendar(FIRST_SETTLEMENT, today + CALENDAR_HORIZON)
            .await?;
        let published = |settlement: NaiveDate| {
            schedule
                .get(&settlement)
                .copied()
                .or_else(|| short_interest_published(settlement, &calendar))
        };
        let stored = self.market.short_interest_settlements().await?;
        let read_through = self.read_through(Dataset::ShortInterest).await?;
        let fresh = settlement_dates(&calendar, today)
            .into_iter()
            .chain(schedule.keys().copied())
            .filter(|settlement| {
                *settlement >= FIRST_SETTLEMENT
                    && !stored.contains(settlement)
                    && read_through.is_none_or(|last| *settlement > last)
            })
            .collect::<BTreeSet<_>>();
        let mut unpublished = None;
        let mut halted = false;
        for (read, settlement) in fresh.into_iter().enumerate() {
            if self.stopping() {
                halted = true;
                break;
            }
            if read == SETTLEMENTS_PER_RUN {
                batch.more_remains(format!("settlements from {settlement}"));
                halted = true;
                break;
            }
            let Some(publication) = published(settlement).filter(|day| *day <= today) else {
                break;
            };
            let file = match finra.short_interest(settlement, publication).await {
                Ok(file) => file,
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    batch.source_error(&settlement.to_string(), &error);
                    halted = true;
                    break;
                }
            };
            self.record_fetched(now, &key, file.records.len()).await?;
            if file.records.is_empty() && publication + SHORT_INTEREST_LATE_AFTER >= today {
                unpublished = Some(settlement);
                break;
            }
            self.store_short_interest(&mut batch, settlement, file)
                .await?;
        }
        if !halted {
            for settlement in stored.iter().rev().take(REREAD_SETTLEMENTS).copied() {
                if self.stopping() {
                    break;
                }
                let mut file = match finra.short_interest(settlement, today).await {
                    Ok(file) => file,
                    Err(error) => {
                        self.record_failure(now, &key, &error).await?;
                        if batch.source_error(&settlement.to_string(), &error) {
                            break;
                        }
                        continue;
                    }
                };
                self.record_fetched(now, &key, file.records.len()).await?;
                let known: BTreeMap<Symbol, ShortInterestRecord> = self
                    .market
                    .short_interest_records(settlement)
                    .await?
                    .into_iter()
                    .map(|record| (record.symbol.clone(), record))
                    .collect();
                file.records.retain(|record| {
                    known
                        .get(&record.symbol)
                        .is_none_or(|stored| !same_position(stored, record))
                });
                if file.records.is_empty() {
                    batch.success(0);
                    continue;
                }
                self.store_short_interest(&mut batch, settlement, file)
                    .await?;
            }
        }
        let work = batch.finish("short interest");
        Ok(match unpublished {
            Some(settlement) if work.outcome == JobOutcome::Noop => Work::waiting(format!(
                "short interest: settlement {settlement} is not published yet"
            )),
            _ => work,
        })
    }

    async fn store_short_interest(
        &self,
        batch: &mut Batch,
        settlement: NaiveDate,
        file: FinraFile<ShortInterestRecord>,
    ) -> Result<(), IngestError> {
        let summary = self
            .market
            .append_short_interest(DataSource::Finra, settlement, file.records)
            .await?;
        batch.success(summary.rows);
        if file.malformed_rows > 0 {
            batch.failure(
                &settlement.to_string(),
                &format!("{} malformed rows skipped", file.malformed_rows),
            );
        }
        Ok(())
    }

    pub(crate) async fn short_volume(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(finra) = &self.sources.finra else {
            return Ok(Work::unconfigured("FINRA"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Finra).await? {
            return Ok(work);
        }
        let today = now.exchange_date();
        let start = self
            .read_through(Dataset::ShortVolume)
            .await?
            .map_or(FIRST_SHORT_VOLUME, |last| last + TimeDelta::days(1));
        let calendar = self.market.calendar(start, today).await?;
        let closed: Vec<NaiveDate> = calendar
            .sessions()
            .iter()
            .filter(|session| session.close <= now)
            .map(|session| session.date)
            .collect();
        let mut batch = Batch::default();
        let mut unpublished = None;
        for (index, date) in closed.iter().copied().enumerate() {
            if self.stopping() {
                break;
            }
            if index == SHORT_VOLUME_FILES_PER_RUN {
                batch.more_remains(format!("trade dates from {date}"));
                break;
            }
            let key = FetchKey::new(
                DataSource::Finra,
                Dataset::ShortVolume,
                None,
                Some((date, date)),
            );
            let file = match finra.short_volume(date).await {
                Ok(Some(file)) => file,
                Ok(None) if date + SHORT_VOLUME_LATE_AFTER < today => FinraFile {
                    records: Vec::new(),
                    malformed_rows: 0,
                },
                Ok(None) => {
                    unpublished = Some(date);
                    break;
                }
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    batch.source_error(&date.to_string(), &error);
                    break;
                }
            };
            self.record_fetched(now, &key, file.records.len()).await?;
            let summary = self
                .market
                .append_short_volume(DataSource::Finra, date, file.records)
                .await?;
            batch.success(summary.rows);
            if file.malformed_rows > 0 {
                batch.failure(
                    &date.to_string(),
                    &format!("{} malformed rows skipped", file.malformed_rows),
                );
            }
        }
        let work = batch.finish("short volume");
        Ok(match unpublished {
            Some(date) if work.outcome == JobOutcome::Noop => {
                Work::waiting(format!("short volume: {date} is not published yet"))
            }
            _ => work,
        })
    }

    async fn read_through(&self, dataset: Dataset) -> Result<Option<NaiveDate>, IngestError> {
        Ok(self
            .market
            .coverage()
            .await?
            .into_iter()
            .find(|row| {
                row.dataset == dataset && row.source == DataSource::Finra && row.symbol.is_none()
            })
            .map(|row| row.last))
    }
}
