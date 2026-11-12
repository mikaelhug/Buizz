use std::collections::BTreeSet;

use buizz_core::market::{DataSource, Dataset, FomcMeeting};
use buizz_core::time::Timestamp;
use buizz_sources::fed::FIRST_YEAR;
use buizz_sources::http::SourceError;
use chrono::Datelike;

use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError};

impl Ingest {
    pub(crate) async fn fomc(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(fed) = &self.sources.fed else {
            return Ok(Work::unconfigured("Federal Reserve"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::FederalReserve).await? {
            return Ok(work);
        }
        let stored: BTreeSet<FomcMeeting> =
            self.market.fomc_meetings().await?.into_iter().collect();
        let mut batch = Batch::default();
        let calendar = fed.calendar().await;
        let Some(first_listed) = self
            .store_fomc(now, &stored, &mut batch, "calendar", calendar)
            .await?
        else {
            return Ok(batch.finish("fomc"));
        };
        let held: BTreeSet<i32> = stored.iter().map(|meeting| meeting.date.year()).collect();
        for year in (FIRST_YEAR..first_listed).filter(|year| !held.contains(year)) {
            let history = fed.history(year).await;
            if self
                .store_fomc(now, &stored, &mut batch, &year.to_string(), history)
                .await?
                .is_none()
            {
                break;
            }
        }
        Ok(batch.finish("fomc"))
    }

    async fn store_fomc(
        &self,
        now: Timestamp,
        stored: &BTreeSet<FomcMeeting>,
        batch: &mut Batch,
        label: &str,
        fetched: Result<Vec<FomcMeeting>, SourceError>,
    ) -> Result<Option<i32>, IngestError> {
        let key = FetchKey::new(DataSource::FederalReserve, Dataset::Fomc, None, None);
        let meetings = match fetched {
            Ok(meetings) => meetings,
            Err(error) => {
                self.record_failure(now, &key, &error).await?;
                batch.source_error(label, &error);
                return Ok(None);
            }
        };
        self.record_fetched(now, &key, meetings.len()).await?;
        let years = meetings.iter().map(|meeting| meeting.date.year());
        let (Some(first), Some(last)) = (years.clone().min(), years.max()) else {
            batch.success(0);
            return Ok(None);
        };
        let fetched: BTreeSet<FomcMeeting> = meetings.into_iter().collect();
        let known: BTreeSet<FomcMeeting> = stored
            .iter()
            .filter(|meeting| (first..=last).contains(&meeting.date.year()))
            .copied()
            .collect();
        let rows = if fetched == known {
            0
        } else {
            self.market
                .replace_fomc_meetings(
                    DataSource::FederalReserve,
                    first..=last,
                    fetched.into_iter().collect(),
                )
                .await?
                .rows
        };
        batch.success(rows);
        Ok(Some(first))
    }
}
