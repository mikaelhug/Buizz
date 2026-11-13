use buizz_core::market::{DataSource, Dataset};
use buizz_core::time::Timestamp;

use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError};

const FILES_PER_RUN: usize = 24;

impl Ingest {
    pub(crate) async fn fails_to_deliver(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(edgar) = &self.sources.edgar else {
            return Ok(Work::unconfigured("EDGAR"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Edgar).await? {
            return Ok(work);
        }
        let key = FetchKey::new(DataSource::Edgar, Dataset::FailsToDeliver, None, None);
        let listed = match edgar.fails_files().await {
            Ok(listed) => listed,
            Err(error) => return self.source_failure(now, &key, &error).await,
        };
        self.record_fetched(now, &key, listed.len()).await?;
        let stored = self.market.fails_to_deliver_files().await?;
        let today = now.exchange_date();
        let mut batch = Batch::default();
        let fresh = listed.iter().filter(|file| !stored.contains(&file.name));
        for (read, file) in fresh.enumerate() {
            if self.stopping() {
                break;
            }
            if read == FILES_PER_RUN {
                batch.more_remains(format!("files from {}", file.name));
                break;
            }
            let archive = match edgar.fails_to_deliver(file, today).await {
                Ok(archive) => archive,
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    if batch.source_error(&file.name, &error) {
                        break;
                    }
                    continue;
                }
            };
            let summary = self
                .market
                .append_fails_to_deliver(
                    DataSource::Edgar,
                    file.name.clone(),
                    today,
                    archive.records,
                )
                .await?;
            batch.success(summary.rows);
            if archive.malformed_rows > 0 {
                batch.failure(
                    &file.name,
                    &format!("{} malformed rows skipped", archive.malformed_rows),
                );
            }
        }
        Ok(batch.finish("fails to deliver"))
    }
}
