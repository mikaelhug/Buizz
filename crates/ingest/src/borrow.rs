use buizz_core::market::{DataSource, Dataset};
use buizz_core::time::Timestamp;

use crate::fetch::{FetchKey, Work};
use crate::{Ingest, IngestError};

impl Ingest {
    pub(crate) async fn borrow(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(ibkr) = &self.sources.ibkr else {
            return Ok(Work::unconfigured("Interactive Brokers"));
        };
        let key = FetchKey::new(DataSource::InteractiveBrokers, Dataset::Borrow, None, None);
        let records = match ibkr.us_borrow().await {
            Ok(records) => records,
            Err(error) => return self.source_failure(now, &key, &error).await,
        };
        self.record_fetched(now, &key, records.len()).await?;
        let Some(at) = records.first().map(|record| record.at) else {
            return Ok(Work::noop("the short stock file lists no US ticker"));
        };
        if self.market.latest_borrow().await? >= Some(at) {
            return Ok(Work::noop(format!(
                "borrow snapshot of {at} already stored"
            )));
        }
        let summary = self
            .market
            .append_borrow_availability(DataSource::InteractiveBrokers, records)
            .await?;
        Ok(Work::ok(
            summary.rows,
            format!("borrow: {} tickers as of {at}", summary.rows),
        ))
    }
}
