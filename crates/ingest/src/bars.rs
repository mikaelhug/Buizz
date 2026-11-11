use buizz_core::market::{DataSource, Dataset, Feed, Symbol, Timeframe};
use buizz_core::time::Timestamp;
use buizz_sources::alpaca::{BarsRequest, HISTORY_START};
use chrono::TimeDelta;

use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError, day_start};

const SYMBOLS_PER_REQUEST: usize = 100;

impl Ingest {
    pub(crate) async fn daily_bars(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(alpaca) = &self.sources.alpaca else {
            return Ok(Work::unconfigured("Alpaca"));
        };
        if let Some(work) = self
            .backoff_work(now, DataSource::Alpaca, Dataset::Bars1d)
            .await?
        {
            return Ok(work);
        }
        let calendar = self
            .market
            .calendar(self.config.history_start, now.exchange_date())
            .await?;
        let Some(target) = calendar
            .sessions()
            .iter()
            .rev()
            .find(|session| session.close <= now)
            .copied()
        else {
            return Ok(Work::noop("no completed session in the calendar"));
        };
        let symbols = self.active_symbols().await?;
        if symbols.is_empty() {
            return Ok(Work::noop("no active instruments"));
        }
        let covered = self
            .symbol_coverage(Dataset::Bars1d, DataSource::Alpaca)
            .await?;
        let floor = self.config.history_start.max(HISTORY_START);
        let mut fresh: Vec<Symbol> = Vec::new();
        let mut lagging: Vec<Symbol> = Vec::new();
        let mut lagging_from = target.date;
        for symbol in symbols {
            match covered.get(&symbol) {
                None => fresh.push(symbol),
                Some(coverage) if coverage.last < target.date => {
                    lagging_from = lagging_from.min(coverage.last + TimeDelta::days(1));
                    lagging.push(symbol);
                }
                Some(_) => {}
            }
        }
        let mut batch = Batch::default();
        'groups: for (from, group) in [(floor, fresh), (lagging_from, lagging)] {
            if from > target.date {
                continue;
            }
            for chunk in group.chunks(SYMBOLS_PER_REQUEST) {
                let request = BarsRequest {
                    symbols: chunk.to_vec(),
                    timeframe: Timeframe::Day,
                    from: day_start(from),
                    to: target.close,
                    feed: Feed::Sip,
                };
                let key = FetchKey::new(
                    DataSource::Alpaca,
                    Dataset::Bars1d,
                    None,
                    Some((from, target.date)),
                );
                let label = format!("{} symbols from {from}", chunk.len());
                match alpaca.bars(&request, &calendar).await {
                    Ok(bars) => {
                        self.record_fetched(now, &key, bars.len()).await?;
                        let summary = self
                            .market
                            .upsert_bars(DataSource::Alpaca, Timeframe::Day, bars)
                            .await?;
                        batch.success(summary.rows);
                    }
                    Err(error) => {
                        self.record_failure(now, &key, &error).await?;
                        if batch.source_error(&label, &error) {
                            break 'groups;
                        }
                    }
                }
            }
        }
        if batch.clean() {
            self.market
                .advance_watermark(Timeframe::Day, target.close)
                .await?;
        }
        Ok(batch.finish("daily bars"))
    }

    pub(crate) async fn restore_daily_watermark(&self) -> Result<(), IngestError> {
        let covered = self
            .symbol_coverage(Dataset::Bars1d, DataSource::Alpaca)
            .await?;
        let Some(last) = covered.values().map(|coverage| coverage.last).max() else {
            return Ok(());
        };
        let calendar = self.market.calendar(last, last).await?;
        if let Some(session) = calendar.session(last) {
            self.market
                .advance_watermark(Timeframe::Day, session.close)
                .await?;
        }
        Ok(())
    }
}
