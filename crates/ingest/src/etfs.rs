use buizz_core::market::{DataSource, Dataset, Symbol};
use buizz_core::time::Timestamp;
use buizz_sources::ssga::ETFS;

use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError};

impl Ingest {
    pub(crate) async fn etf_holdings(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(ssga) = &self.sources.ssga else {
            return Ok(Work::unconfigured("State Street"));
        };
        let stored = self.market.latest_etf_dates().await?;
        let mut batch = Batch::default();
        for etf in ETFS.map(Symbol::new) {
            if self.stopping() {
                batch.more_remains(format!("ETF holdings from {etf}"));
                break;
            }
            let stored = stored.get(&etf).copied().unwrap_or_default();
            let key = FetchKey::new(
                DataSource::StateStreet,
                Dataset::EtfHoldings,
                Some(&etf),
                None,
            );
            let fetched = match ssga.holdings(&etf).await {
                Ok(file) => match ssga.shares(&etf).await {
                    Ok(shares) => Ok((file, shares)),
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            };
            let (file, shares) = match fetched {
                Ok(fetched) => fetched,
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    if batch.source_error(etf.as_str(), &error) {
                        break;
                    }
                    continue;
                }
            };
            let file = Some(file).filter(|file| stored.holdings < Some(file.as_of));
            let shares: Vec<_> = shares
                .into_iter()
                .filter(|day| stored.shares < Some(day.date))
                .collect();
            let rows = file.as_ref().map_or(0, |file| file.holdings.len()) + shares.len();
            self.record_fetched(now, &key, rows).await?;
            let summary = self
                .market
                .append_etf_holdings(DataSource::StateStreet, now, etf, file, shares)
                .await?;
            batch.success(summary.rows);
        }
        Ok(batch.finish("ETF holdings"))
    }
}
