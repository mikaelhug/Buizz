use std::collections::BTreeSet;

use buizz_core::market::{DataSource, Dataset, TradingHaltRecord};
use buizz_core::time::Timestamp;

use crate::fetch::{FetchKey, Work};
use crate::{Ingest, IngestError};

impl Ingest {
    pub(crate) async fn trading_halts(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(nasdaq) = &self.sources.nasdaq else {
            return Ok(Work::unconfigured("Nasdaq Trader"));
        };
        let key = FetchKey::new(DataSource::Nasdaq, Dataset::TradingHalts, None, None);
        let listed = match nasdaq.trading_halts().await {
            Ok(halts) => halts,
            Err(error) => return self.source_failure(now, &key, &error).await,
        };
        self.record_fetched(now, &key, listed.len()).await?;
        let Some(since) = listed.iter().map(|halt| halt.halted_at).min() else {
            return self.halts_seen(now).await;
        };
        let stored = self.market.latest_trading_halts(since).await?;
        let changed: Vec<TradingHaltRecord> = listed
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|halt| !stored.contains(halt))
            .collect();
        if changed.is_empty() {
            return self.halts_seen(now).await;
        }
        let summary = self
            .market
            .append_trading_halts(DataSource::Nasdaq, now, changed)
            .await?;
        Ok(Work::ok(
            summary.rows,
            format!("trading halts: {} new or changed", summary.rows),
        ))
    }

    async fn halts_seen(&self, now: Timestamp) -> Result<Work, IngestError> {
        let today = now.exchange_date();
        let through = self
            .market
            .coverage()
            .await?
            .into_iter()
            .find(|row| {
                row.dataset == Dataset::TradingHalts
                    && row.source == DataSource::Nasdaq
                    && row.symbol.is_none()
            })
            .map(|row| row.last);
        if through < Some(today) {
            self.market
                .record_seen(DataSource::Nasdaq, Dataset::TradingHalts, today)
                .await?;
        }
        Ok(Work::noop("trading halts: nothing new"))
    }
}
