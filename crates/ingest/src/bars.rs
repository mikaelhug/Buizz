use std::collections::{BTreeMap, BTreeSet};

use buizz_core::market::{DataSource, Dataset, Feed, Symbol, Timeframe};
use buizz_core::ops::FetchStatus;
use buizz_core::time::Timestamp;
use buizz_sources::alpaca::{BarsRequest, HISTORY_START, SIP_DELAY};
use buizz_sources::http::SourceError;
use chrono::{NaiveDate, TimeDelta};

use crate::fetch::{Batch, FetchKey, Work, read, waiting};
use crate::{CORE_ETFS, Ingest, IngestError, Priced, day_start};

const SYMBOLS_PER_REQUEST: usize = 1_000;
const REQUESTS_PER_RUN: usize = 600;
const SETTLE_MARGIN: TimeDelta = TimeDelta::minutes(15);
const SETTLING: TimeDelta = TimeDelta::hours(24);
const MAX_LAG_SESSIONS: usize = 20;
const REFUSED_REQUEST: [u16; 2] = [400, 422];
const DORMANT_RETRY: TimeDelta = TimeDelta::days(7);
const INDEX_RANKS: usize = CORE_ETFS.len() + 1;
const ALONE_PER_RUN: usize = 20;

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
            return Ok(Work::waiting("no settled session in the calendar"));
        };
        let target = sessions[target_index];
        let closed = &sessions[..=target_index];
        let settling = closed.partition_point(|session| session.close + SETTLING <= now);
        let settling_from = closed.get(settling).map(|session| session.date);
        let current_from = settling_from.unwrap_or(target.date);
        let next_session = |last: NaiveDate| {
            let next = closed.partition_point(|session| session.date <= last);
            closed.get(next).map(|session| session.date)
        };
        let dormant_before = sessions[target_index.saturating_sub(MAX_LAG_SESSIONS)].date;
        let symbols = self.priced_symbols().await?;
        if symbols.is_empty() {
            return Ok(Work::waiting("no instruments to price"));
        }
        let covered = self
            .symbol_coverage(Dataset::Bars1d, DataSource::Alpaca)
            .await?;
        let refused: BTreeSet<Symbol> = self
            .state
            .latest_rejections(DataSource::Alpaca, Dataset::Bars1d)
            .await?
            .into_iter()
            .filter(|fetch| waiting(fetch, now))
            .filter_map(|fetch| fetch.symbol)
            .collect();
        let floor = self.config.history_start.max(HISTORY_START);
        let mut groups: BTreeMap<NaiveDate, Vec<Symbol>> = BTreeMap::new();
        let mut dormant: BTreeSet<Symbol> = BTreeSet::new();
        let mut alone: Vec<(NaiveDate, Vec<Symbol>)> = Vec::new();
        for Priced {
            symbol,
            active,
            rank,
        } in symbols
        {
            if refused.contains(&symbol) {
                continue;
            }
            let last = covered.get(&symbol).map(|coverage| coverage.last);
            let from = match last {
                Some(last) if last >= dormant_before => match (settling_from, next_session(last)) {
                    (Some(settling), next) => next.map_or(settling, |next| next.min(settling)),
                    (None, Some(next)) => next,
                    (None, None) => continue,
                },
                _ if self.dormant_checked(now, &symbol, active).await? => {
                    if last.is_none()
                        && rank <= INDEX_RANKS
                        && alone.len() < ALONE_PER_RUN
                        && self.empty_only_among_many(&symbol).await?
                    {
                        dormant.insert(symbol.clone());
                        alone.push((floor, vec![symbol]));
                    }
                    continue;
                }
                _ => {
                    dormant.insert(symbol.clone());
                    last.map_or(floor, |last| last + TimeDelta::days(1))
                }
            };
            groups.entry(from).or_default().push(symbol);
        }
        let mut groups: Vec<(NaiveDate, Vec<Symbol>)> = groups.into_iter().collect();
        groups.sort_by_key(|(from, _)| (*from != current_from, *from));
        groups.extend(alone);
        let mut batch = Batch::default();
        let mut requests = 0;
        let mut current = true;
        'groups: for (from, group) in groups {
            let in_current = from == current_from;
            let mut pending: Vec<Vec<Symbol>> = group
                .chunks(SYMBOLS_PER_REQUEST)
                .rev()
                .map(<[Symbol]>::to_vec)
                .collect();
            'chunks: while let Some(chunk) = pending.pop() {
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
                        current &= !in_current;
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
                        Err(error) if refused_request(&error) && chunk.len() > 1 => {
                            let (left, right) = chunk.split_at(chunk.len() / 2);
                            pending.push(right.to_vec());
                            pending.push(left.to_vec());
                            continue 'chunks;
                        }
                        Err(error) if refused_request(&error) => {
                            let symbol = &chunk[0];
                            let key = FetchKey::new(
                                DataSource::Alpaca,
                                Dataset::Bars1d,
                                Some(symbol),
                                Some((from, target.date)),
                            );
                            self.record_rejection(now, &key, &error.to_string()).await?;
                            batch.failure(symbol.as_str(), &error);
                            continue 'chunks;
                        }
                        Err(error) => {
                            self.record_failure(now, &key, &error).await?;
                            current &= !in_current;
                            let label = format!("{} symbols from {from}", chunk.len());
                            if batch.source_error(&label, &error) {
                                break 'groups;
                            }
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
        if current {
            self.market
                .advance_watermark(Timeframe::Day, target.close)
                .await?;
        }
        Ok(batch.finish("daily bars"))
    }

    async fn empty_only_among_many(&self, symbol: &Symbol) -> Result<bool, IngestError> {
        let fetches = self
            .state
            .recent_fetches(DataSource::Alpaca, Dataset::Bars1d, Some(symbol.clone()), 2)
            .await?;
        Ok(matches!(fetches.as_slice(), [only] if only.status == FetchStatus::Empty))
    }

    async fn dormant_checked(
        &self,
        now: Timestamp,
        symbol: &Symbol,
        active: bool,
    ) -> Result<bool, IngestError> {
        let latest = self
            .latest_fetch(DataSource::Alpaca, Dataset::Bars1d, symbol)
            .await?;
        Ok(latest.is_some_and(|fetch| read(&fetch) && (!active || fetch.at + DORMANT_RETRY > now)))
    }
}

fn refused_request(error: &SourceError) -> bool {
    match error {
        SourceError::Decode(_) => true,
        SourceError::Status { status, .. } => REFUSED_REQUEST.contains(status),
        _ => false,
    }
}
