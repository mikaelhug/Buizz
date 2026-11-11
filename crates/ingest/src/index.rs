use buizz_core::market::{DataSource, Dataset, StockIndex};
use buizz_core::time::Timestamp;

use crate::fetch::{FetchKey, Work};
use crate::{Ingest, IngestError};

impl Ingest {
    pub(crate) async fn index_membership(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(wikipedia) = &self.sources.wikipedia else {
            return Ok(Work::unconfigured("Wikipedia"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Wikipedia).await? {
            return Ok(work);
        }
        let key = FetchKey::new(DataSource::Wikipedia, Dataset::IndexMembership, None, None);
        let today = now.exchange_date();
        let history = match wikipedia.sp500(today).await {
            Ok(history) => history,
            Err(error) => return self.source_failure(now, &key, &error).await,
        };
        self.record_fetched(now, &key, history.members.len())
            .await?;
        let stored = self.market.index_history(StockIndex::Sp500).await?;
        if stored.as_ref() == Some(&history) {
            return Ok(Work::noop("S&P 500 membership unchanged"));
        }
        let message = format!(
            "S&P 500: {} membership spans from {} changes, reliable from {}",
            history.members.len(),
            history.changes,
            history
                .reliable_from
                .map_or_else(|| "no year".to_owned(), |date| date.to_string())
        );
        let summary = self
            .market
            .replace_index_history(DataSource::Wikipedia, history, today)
            .await?;
        Ok(Work::ok(summary.rows, message))
    }
}
