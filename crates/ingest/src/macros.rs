use std::collections::HashSet;

use buizz_core::market::{DataSource, Dataset, MacroObservation, MacroSeries};
use buizz_core::time::Timestamp;

use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError};

impl Ingest {
    pub(crate) async fn macro_series(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(fred) = &self.sources.fred else {
            return Ok(Work::unconfigured("FRED"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Fred).await? {
            return Ok(work);
        }
        let mut batch = Batch::default();
        let mut changed: Vec<MacroObservation> = Vec::new();
        for series in MacroSeries::FRED {
            let key = FetchKey::new(DataSource::Fred, Dataset::Macro, None, None);
            match fred.vintages(series).await {
                Ok(vintages) => {
                    self.record_fetched(now, &key, vintages.len()).await?;
                    batch.success(0);
                    let stored: HashSet<MacroObservation> = self
                        .market
                        .macro_observations(series)
                        .await?
                        .into_iter()
                        .collect();
                    changed.extend(
                        vintages
                            .into_iter()
                            .filter(|vintage| !stored.contains(vintage)),
                    );
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
        if !changed.is_empty() {
            let summary = self
                .market
                .upsert_macro_observations(DataSource::Fred, changed)
                .await?;
            batch.success(summary.rows);
        }
        Ok(batch.finish("macro"))
    }
}
