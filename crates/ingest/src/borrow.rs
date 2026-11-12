use buizz_core::market::{DataSource, Dataset};
use buizz_core::time::Timestamp;
use buizz_sources::ibkr::BorrowFile;

use crate::fetch::{FetchKey, Work};
use crate::{Ingest, IngestError};

impl Ingest {
    pub(crate) async fn borrow(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(ibkr) = &self.sources.ibkr else {
            return Ok(Work::unconfigured("Interactive Brokers"));
        };
        let key = FetchKey::new(DataSource::InteractiveBrokers, Dataset::Borrow, None, None);
        let BorrowFile {
            records,
            malformed_rows,
        } = match ibkr.us_borrow().await {
            Ok(file) => file,
            Err(error) => return self.source_failure(now, &key, &error).await,
        };
        self.record_fetched(now, &key, records.len()).await?;
        let Some(at) = records.first().map(|record| record.at) else {
            return Ok(Work::noop("the short stock file lists no US ticker"));
        };
        if self.market.latest_borrow().await? >= Some(at) {
            return Ok(Work::waiting(format!(
                "borrow snapshot of {at} already stored"
            )));
        }
        let summary = self
            .market
            .append_borrow_availability(DataSource::InteractiveBrokers, records)
            .await?;
        let message = format!("borrow: {} tickers as of {at}", summary.rows);
        Ok(match malformed_rows {
            0 => Work::ok(summary.rows, message),
            rows => Work::partial(
                summary.rows,
                format!("{message}, {rows} malformed rows skipped"),
            ),
        })
    }
}
