use buizz_core::market::{DataSource, Dataset};
use buizz_core::time::Timestamp;
use chrono::{Datelike, TimeDelta, Weekday};

use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError};

const FIRST_LOOKBACK: TimeDelta = TimeDelta::days(7);
const PUBLISHED_WITHIN: TimeDelta = TimeDelta::days(3);

impl Ingest {
    pub(crate) async fn sec_index(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(edgar) = &self.sources.edgar else {
            return Ok(Work::unconfigured("SEC EDGAR"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Edgar).await? {
            return Ok(work);
        }
        let today = now.exchange_date();
        let read_through = self
            .market
            .coverage()
            .await?
            .into_iter()
            .find(|row| row.dataset == Dataset::SecFilings && row.symbol.is_none())
            .map(|row| row.last);
        let mut day = read_through.map_or(today - FIRST_LOOKBACK, |last| last + TimeDelta::days(1));
        let mut batch = Batch::default();
        while day <= today && !self.stopping() {
            if matches!(day.weekday(), Weekday::Sat | Weekday::Sun) {
                day += TimeDelta::days(1);
                continue;
            }
            let key = FetchKey::new(
                DataSource::Edgar,
                Dataset::SecFilings,
                None,
                Some((day, day)),
            );
            match edgar.daily_index(day).await {
                Ok(Some(filings)) => {
                    let fetched = filings.len();
                    let summary = self
                        .market
                        .append_sec_filings(DataSource::Edgar, filings, day)
                        .await?;
                    self.record_fetched(now, &key, fetched).await?;
                    batch.success(summary.rows);
                }
                Ok(None) if day + PUBLISHED_WITHIN < today => {
                    self.market
                        .append_sec_filings(DataSource::Edgar, Vec::new(), day)
                        .await?;
                    self.record_fetched(now, &key, 0).await?;
                    batch.success(0);
                }
                Ok(None) => {}
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    batch.source_error(&day.to_string(), &error);
                    break;
                }
            }
            day += TimeDelta::days(1);
        }
        Ok(batch.finish("SEC filing index"))
    }
}
