use std::collections::BTreeMap;

use buizz_core::market::{DataSource, Dataset, Feed, Symbol, Timeframe};
use buizz_core::time::Timestamp;
use buizz_sources::alpaca::BarsRequest;
use buizz_sources::http::SourceError;
use chrono::{NaiveDate, TimeDelta};

use crate::fetch::FetchKey;
use crate::{Ingest, IngestError, day_start};

const SYMBOLS_PER_REQUEST: usize = 50;

impl Ingest {
    pub async fn ensure_minute_coverage(
        &self,
        symbols: &[Symbol],
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<(), IngestError> {
        let alpaca = self
            .sources
            .alpaca
            .as_ref()
            .ok_or(IngestError::Unconfigured("Alpaca"))?;
        let now = Timestamp::now();
        if let Some(until) = self
            .rate_limited_until(now, DataSource::Alpaca)
            .await?
        {
            return Err(SourceError::RateLimited {
                retry_after: (until - now).to_std().ok(),
            }
            .into());
        }
        let gaps = self
            .market
            .coverage_gaps(symbols.to_vec(), Timeframe::Minute, from, to)
            .await?;
        let calendar = self.market.calendar(from, to).await?;
        let mut grouped: BTreeMap<(NaiveDate, NaiveDate), Vec<Symbol>> = BTreeMap::new();
        for gap in gaps {
            grouped
                .entry((gap.from, gap.to))
                .or_default()
                .push(gap.symbol);
        }
        for ((gap_from, gap_to), group) in grouped {
            for chunk in group.chunks(SYMBOLS_PER_REQUEST) {
                let request = BarsRequest {
                    symbols: chunk.to_vec(),
                    timeframe: Timeframe::Minute,
                    from: day_start(gap_from),
                    to: day_start(gap_to + TimeDelta::days(1)),
                    feed: Feed::Sip,
                };
                let key = FetchKey::new(
                    DataSource::Alpaca,
                    Dataset::Bars1m,
                    None,
                    Some((gap_from, gap_to)),
                );
                match alpaca.bars(&request, &calendar).await {
                    Ok(bars) => {
                        self.record_fetched(now, &key, bars.len()).await?;
                        self.market
                            .upsert_bars(DataSource::Alpaca, Timeframe::Minute, bars)
                            .await?;
                    }
                    Err(error) => {
                        self.record_failure(now, &key, &error).await?;
                        return Err(error.into());
                    }
                }
            }
        }
        Ok(())
    }
}
