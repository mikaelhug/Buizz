use std::collections::{BTreeMap, BTreeSet};

use buizz_core::market::{DataSource, Dataset, Feed, Symbol, Timeframe};
use buizz_core::ops::FetchStatus;
use buizz_core::time::Timestamp;
use buizz_sources::alpaca::{BarsRequest, HISTORY_START, SIP_DELAY};
use chrono::{NaiveDate, TimeDelta};

use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError, Priced, day_start};

const SYMBOLS_PER_REQUEST: usize = 1_000;
const REQUESTS_PER_RUN: usize = 600;
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
        let symbols = self.priced_symbols().await?;
        if symbols.is_empty() {
            return Ok(Work::noop("no instruments to price"));
        }
        let covered = self
            .symbol_coverage(Dataset::Bars1d, DataSource::Alpaca)
            .await?;
        let floor = self.config.history_start.max(HISTORY_START);
        let mut groups: BTreeMap<NaiveDate, Vec<Symbol>> = BTreeMap::new();
        let mut dormant: BTreeSet<Symbol> = BTreeSet::new();
        for Priced { symbol, active, .. } in symbols {
            let last = covered.get(&symbol).map(|coverage| coverage.last);
            let from = match last {
                Some(last) if last >= dormant_before => revised_from.min(last + TimeDelta::days(1)),
                _ if self.dormant_checked(now, &symbol, active).await? => continue,
                _ => {
                    dormant.insert(symbol.clone());
                    last.map_or(floor, |last| last + TimeDelta::days(1))
                }
            };
            groups.entry(from).or_default().push(symbol);
        }
        let mut groups: Vec<(NaiveDate, Vec<Symbol>)> = groups.into_iter().collect();
        groups.sort_by_key(|(from, _)| (*from != revised_from, *from));
        let mut batch = Batch::default();
        let mut requests = 0;
        let mut current = true;
        'groups: for (from, group) in groups {
            'chunks: for chunk in group.chunks(SYMBOLS_PER_REQUEST) {
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
                let mut received: BTreeMap<Symbol, usize> = BTreeMap::new();
                let mut token = None;
                loop {
                    if requests == REQUESTS_PER_RUN || self.stopping() {
                        batch.more_remains("symbols");
                        current &= from != revised_from;
                        break 'groups;
                    }
                    requests += 1;
                    match alpaca.bars_page(&request, &calendar, token.take()).await {
                        Ok(page) => {
                            let fetched = page.bars.len();
                            for bar in &page.bars {
                                *received.entry(bar.symbol.clone()).or_default() += 1;
                            }
                            let rows = self
                                .market
                                .upsert_bars(DataSource::Alpaca, Timeframe::Day, page.bars)
                                .await?
                                .rows;
                            self.record_fetched(now, &key, fetched).await?;
                            batch.success(rows);
                            match page.next {
                                Some(next) => token = Some(next),
                                None => break,
                            }
                        }
                        Err(error) => {
                            self.record_failure(now, &key, &error).await?;
                            let label = format!("{} symbols from {from}", chunk.len());
                            if batch.source_error(&label, &error) {
                                break 'groups;
                            }
                            current &= from != revised_from;
                            continue 'chunks;
                        }
                    }
                }
                for symbol in chunk.iter().filter(|symbol| dormant.contains(*symbol)) {
                    let key = FetchKey::new(
                        DataSource::Alpaca,
                        Dataset::Bars1d,
                        Some(symbol),
                        Some((from, target.date)),
                    );
                    let rows = received.get(symbol).copied().unwrap_or_default();
                    self.record_fetched(now, &key, rows).await?;
                }
            }
        }
        if current && batch.clean() {
            self.market
                .advance_watermark(Timeframe::Day, target.close)
                .await?;
        }
        Ok(batch.finish("daily bars"))
    }

    async fn dormant_checked(
        &self,
        now: Timestamp,
        symbol: &Symbol,
        active: bool,
    ) -> Result<bool, IngestError> {
        let latest = self
            .state
            .recent_fetches(DataSource::Alpaca, Dataset::Bars1d, Some(symbol.clone()), 1)
            .await?;
        Ok(latest.first().is_some_and(|fetch| {
            matches!(fetch.status, FetchStatus::Ok | FetchStatus::Empty)
                && (!active || fetch.at + DORMANT_RETRY > now)
        }))
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
