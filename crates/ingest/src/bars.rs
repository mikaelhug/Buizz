use std::collections::{BTreeMap, BTreeSet};

use buizz_core::market::{Bar, DataSource, Dataset, Feed, Symbol, Timeframe};
use buizz_core::ops::FetchStatus;
use buizz_core::time::Timestamp;
use buizz_sources::alpaca::{BarsRequest, HISTORY_START, SIP_DELAY};
use chrono::{NaiveDate, TimeDelta};

use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError, day_start};

const SYMBOLS_PER_REQUEST: usize = 100;
const SETTLE_MARGIN: TimeDelta = TimeDelta::minutes(15);
const REVISED_SESSIONS: usize = 2;
const MAX_LAG_SESSIONS: usize = 20;
const DORMANT_RETRY: TimeDelta = TimeDelta::days(7);

impl Ingest {
    pub(crate) async fn daily_bars(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(alpaca) = &self.sources.alpaca else {
            return Ok(Work::unconfigured("Alpaca"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Alpaca).await? {
            return Ok(work);
        }
        let calendar = self
            .market
            .calendar(self.config.history_start, now.exchange_date())
            .await?;
        let sip_delay = TimeDelta::from_std(SIP_DELAY).expect("the SIP delay fits a time delta");
        let settled = now - sip_delay - SETTLE_MARGIN;
        let sessions = calendar.sessions();
        let Some(target_index) = sessions
            .iter()
            .rposition(|session| session.close <= settled)
        else {
            return Ok(Work::noop("no settled session in the calendar"));
        };
        let target = sessions[target_index];
        let revised_from = sessions[target_index.saturating_sub(REVISED_SESSIONS - 1)].date;
        let dormant_before = sessions[target_index.saturating_sub(MAX_LAG_SESSIONS)].date;
        let symbols = self.active_symbols().await?;
        if symbols.is_empty() {
            return Ok(Work::noop("no active instruments"));
        }
        let covered = self
            .symbol_coverage(Dataset::Bars1d, DataSource::Alpaca)
            .await?;
        let floor = self.config.history_start.max(HISTORY_START);
        let mut groups: BTreeMap<NaiveDate, Vec<Symbol>> = BTreeMap::new();
        let mut dormant: BTreeSet<Symbol> = BTreeSet::new();
        for symbol in symbols {
            let last = covered.get(&symbol).map(|coverage| coverage.last);
            let from = match last {
                Some(last) if last >= dormant_before => revised_from.min(last + TimeDelta::days(1)),
                _ if self.dormant_checked(now, &symbol).await? => continue,
                _ => {
                    dormant.insert(symbol.clone());
                    last.map_or(floor, |last| last + TimeDelta::days(1))
                }
            };
            groups.entry(from).or_default().push(symbol);
        }
        let mut batch = Batch::default();
        'groups: for (from, group) in groups {
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
                        for symbol in chunk.iter().filter(|symbol| dormant.contains(*symbol)) {
                            let rows = bars.iter().filter(|bar| bar.symbol == *symbol).count();
                            let key = FetchKey::new(
                                DataSource::Alpaca,
                                Dataset::Bars1d,
                                Some(symbol),
                                Some((from, target.date)),
                            );
                            self.record_fetched(now, &key, rows).await?;
                        }
                        let changed = self.changed_daily_bars(chunk, from, bars).await?;
                        let rows = if changed.is_empty() {
                            0
                        } else {
                            self.market
                                .upsert_bars(DataSource::Alpaca, Timeframe::Day, changed)
                                .await?
                                .rows
                        };
                        batch.success(rows);
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

    async fn dormant_checked(&self, now: Timestamp, symbol: &Symbol) -> Result<bool, IngestError> {
        let latest = self
            .state
            .recent_fetches(DataSource::Alpaca, Dataset::Bars1d, Some(symbol.clone()), 1)
            .await?;
        Ok(latest.first().is_some_and(|fetch| {
            matches!(fetch.status, FetchStatus::Ok | FetchStatus::Empty)
                && fetch.at + DORMANT_RETRY > now
        }))
    }

    async fn changed_daily_bars(
        &self,
        symbols: &[Symbol],
        from: NaiveDate,
        bars: Vec<Bar>,
    ) -> Result<Vec<Bar>, IngestError> {
        let Some(last) = bars.iter().map(|bar| bar.start.exchange_date()).max() else {
            return Ok(bars);
        };
        let stored: BTreeMap<(Symbol, Timestamp), Bar> = self
            .market
            .bars_between(
                Timeframe::Day,
                symbols.to_vec(),
                day_start(from),
                day_start(last + TimeDelta::days(1)),
            )
            .await?
            .into_iter()
            .map(|bar| ((bar.symbol.clone(), bar.start), bar))
            .collect();
        Ok(bars
            .into_iter()
            .filter(|bar| stored.get(&(bar.symbol.clone(), bar.start)) != Some(bar))
            .collect())
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
