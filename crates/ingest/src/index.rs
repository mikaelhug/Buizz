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
        let parsed = match wikipedia.sp500(today).await {
            Ok(parsed) => parsed,
            Err(error) => return self.source_failure(now, &key, &error).await,
        };
        let history = parsed.history;
        let stored = self.market.index_history(StockIndex::Sp500).await?;
        if let Some(stored) = &stored
            && u64::from(history.changes) * 10 < u64::from(stored.changes) * 9
        {
            let message = format!(
                "S&P 500: the change log shrank from {} to {} changes, keeping the stored history",
                stored.changes, history.changes
            );
            self.record_rejection(now, &key, &message).await?;
            return Ok(Work::failed(message));
        }
        self.record_fetched(now, &key, history.members.len())
            .await?;
        let unparsed = match parsed.unparsed_rows {
            0 => String::new(),
            rows => format!(", {rows} table rows unparsed"),
        };
        if stored.as_ref() == Some(&history) {
            return Ok(Work::noop(format!(
                "S&P 500 membership unchanged{unparsed}"
            )));
        }
        let message = format!(
            "S&P 500: {} membership spans from {} changes, reliable from {}{unparsed}",
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
        Ok(match parsed.unparsed_rows {
            0 => Work::ok(summary.rows, message),
            _ => Work::partial(summary.rows, message),
        })
    }
}
