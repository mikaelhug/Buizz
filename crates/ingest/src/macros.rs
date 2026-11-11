use std::collections::HashSet;

use buizz_core::market::{DataSource, Dataset, MacroObservation, MacroSeries};
use buizz_core::time::Timestamp;
use chrono::TimeDelta;

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
        for series in MacroSeries::FRED {
            let key = FetchKey::new(DataSource::Fred, Dataset::Macro, None, None);
            let stored: HashSet<MacroObservation> = self
                .market
                .macro_observations(series)
                .await?
                .into_iter()
                .collect();
            if let Some(last) = stored.iter().map(|observation| observation.published).max() {
                match fred
                    .vintage_dates_since(series, last + TimeDelta::days(1))
                    .await
                {
                    Ok(dates) if dates.is_empty() => {
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
}
