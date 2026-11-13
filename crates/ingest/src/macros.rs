use std::collections::{BTreeMap, HashSet};

use buizz_core::market::{DataSource, Dataset, MacroObservation, MacroSeries, Symbol};
use buizz_core::time::Timestamp;
use chrono::NaiveDate;

use crate::fetch::{Batch, FetchKey, Work, read};
use crate::{Ingest, IngestError, schedule};

pub(crate) fn series_key(series: MacroSeries) -> Symbol {
    Symbol::new(&format!("{series:?}"))
}

impl Ingest {
    pub(crate) async fn series_unread(&self, job: &str) -> Result<bool, IngestError> {
        let (source, series): (DataSource, &[MacroSeries]) = match job {
            schedule::MACRO => (DataSource::Fred, &MacroSeries::FRED),
            schedule::VIX => (DataSource::Cboe, &MacroSeries::CBOE),
            _ => return Ok(false),
        };
        for series in series {
            let fetch = self
                .latest_fetch(source, Dataset::Macro, &series_key(*series))
                .await?;
            if !fetch.is_some_and(|fetch| read(&fetch)) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) async fn macro_series(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(fred) = &self.sources.fred else {
            return Ok(Work::unconfigured("FRED"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Fred).await? {
            return Ok(work);
        }
        let mut batch = Batch::default();
        for series in MacroSeries::FRED {
            let symbol = series_key(series);
            let key = FetchKey::new(DataSource::Fred, Dataset::Macro, Some(&symbol), None);
            let stored: HashSet<MacroObservation> = self
                .market
                .macro_observations(series)
                .await?
                .into_iter()
                .collect();
            if let Some(last) = stored.iter().map(|observation| observation.published).max() {
                match fred.vintage_dates_since(series, last).await {
                    Ok(dates) if dates.iter().all(|date| *date <= last) => {
                        self.record_fetched(now, &key, 0).await?;
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

    pub(crate) async fn vix(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(cboe) = &self.sources.cboe else {
            return Ok(Work::unconfigured("CBOE"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Cboe).await? {
            return Ok(work);
        }
        let today = now.exchange_date();
        let mut batch = Batch::default();
        for series in MacroSeries::CBOE {
            let symbol = series_key(series);
            let key = FetchKey::new(DataSource::Cboe, Dataset::Macro, Some(&symbol), None);
            let closes = match cboe.daily_closes(series).await {
                Ok(closes) => closes,
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    if batch.source_error(&format!("{series:?}"), &error) {
                        break;
                    }
                    continue;
                }
            };
            self.record_fetched(now, &key, closes.len()).await?;
            let latest: BTreeMap<NaiveDate, MacroObservation> = self
                .market
                .macro_observations(series)
                .await?
                .into_iter()
                .map(|observation| (observation.date, observation))
                .collect();
            let changed: Vec<MacroObservation> = closes
                .into_iter()
                .filter_map(|close| match latest.get(&close.date) {
                    None => Some(close),
                    Some(known) if known.value == close.value => None,
                    Some(_) => Some(MacroObservation {
                        published: today.max(close.date),
                        ..close
                    }),
                })
                .collect();
            let rows = if changed.is_empty() {
                0
            } else {
                self.market
                    .upsert_macro_observations(DataSource::Cboe, changed)
                    .await?
                    .rows
            };
            batch.success(rows);
        }
        Ok(batch.finish("vix"))
    }
}
