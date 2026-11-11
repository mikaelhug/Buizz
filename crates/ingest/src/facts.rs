use buizz_core::market::{AssetClass, DataSource, Dataset, Symbol};
use buizz_core::ops::FetchStatus;
use buizz_core::time::Timestamp;
use chrono::TimeDelta;

use crate::fetch::{Batch, FetchKey, Work};
use crate::reference::CALENDAR_HORIZON_DAYS;
use crate::{Ingest, IngestError};

const COMPANIES_PER_RUN: usize = 1_000;
const REFRESH_AFTER: TimeDelta = TimeDelta::days(7);
pub(crate) const NO_CIK: &str = "no SEC CIK for this ticker";

impl Ingest {
    pub(crate) async fn fundamentals(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(edgar) = &self.sources.edgar else {
            return Ok(Work::unconfigured("SEC EDGAR"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Edgar).await? {
            return Ok(work);
        }
        let due = self.companies_due(now, Dataset::Fundamentals).await?;
        if due.is_empty() {
            return Ok(Work::noop("no company needs a refresh"));
        }
        let map_key = FetchKey::new(DataSource::Edgar, Dataset::Fundamentals, None, None);
        let tickers = match edgar.ticker_map().await {
            Ok(tickers) => tickers,
            Err(error) => return self.source_failure(now, &map_key, &error).await,
        };
        let from = self.config.history_start;
        let to = now.exchange_date() + TimeDelta::days(CALENDAR_HORIZON_DAYS);
        let calendar = self.market.calendar(from, to).await?;
        let mut batch = Batch::default();
        let mut without_cik = 0;
        for symbol in due {
            let key = FetchKey::new(
                DataSource::Edgar,
                Dataset::Fundamentals,
                Some(&symbol),
                None,
            );
            let Some(cik) = tickers.get(&symbol) else {
                self.record_rejection(now, &key, NO_CIK).await?;
                without_cik += 1;
                continue;
            };
            match edgar.company_facts(&symbol, *cik, &calendar).await {
                Ok(facts) => {
                    let fetched = facts.len();
                    let summary = self
                        .market
                        .append_fundamental_facts(DataSource::Edgar, facts)
                        .await?;
                    self.record_fetched(now, &key, fetched).await?;
                    batch.success(summary.rows);
                }
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    if batch.source_error(symbol.as_str(), &error) {
                        break;
                    }
                }
            }
        }
        let mut work = batch.finish("fundamentals");
        if without_cik > 0 {
            work.message
                .push_str(&format!(" ({without_cik} tickers without a CIK)"));
        }
        Ok(work)
    }

    pub(crate) async fn companies_due(
        &self,
        now: Timestamp,
        dataset: Dataset,
    ) -> Result<Vec<Symbol>, IngestError> {
        let mut due: Vec<Symbol> = Vec::new();
        for instrument in self.active_instruments().await? {
            if due.len() == COMPANIES_PER_RUN {
                break;
            }
            if instrument.asset_class == AssetClass::Stock
                && self.company_due(now, dataset, &instrument.symbol).await?
            {
                due.push(instrument.symbol);
            }
        }
        Ok(due)
    }

    async fn company_due(
        &self,
        now: Timestamp,
        dataset: Dataset,
        symbol: &Symbol,
    ) -> Result<bool, IngestError> {
        let latest = self
            .state
            .recent_fetches(DataSource::Edgar, dataset, Some(symbol.clone()), 1)
            .await?;
        Ok(latest.first().is_none_or(|fetch| {
            let waiting = fetch.retry_after.is_some_and(|until| until > now);
            let fresh = matches!(fetch.status, FetchStatus::Ok | FetchStatus::Empty)
                && fetch.at + REFRESH_AFTER > now;
            !waiting && !fresh
        }))
    }
}
