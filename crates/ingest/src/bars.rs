use std::collections::{BTreeMap, BTreeSet};

use buizz_core::market::{
    AssetClass, Bar, Calendar, DataSource, Dataset, Feed, Instrument, Session, StockIndex, Symbol,
    Timeframe,
};
use buizz_core::ops::{CoverageGap, FetchStatus};
use buizz_core::time::Timestamp;
use buizz_sources::alpaca::{AlpacaClient, BarsRequest, HISTORY_START, SIP_DELAY, Tickers};
use buizz_sources::http::SourceError;
use chrono::{NaiveDate, TimeDelta};

use crate::fetch::{Batch, FetchKey, Work, read, waiting};
use crate::{CORE_ETFS, Ingest, IngestError, Priced, day_start};

const SYMBOLS_PER_REQUEST: usize = 1_000;
const REQUESTS_PER_RUN: usize = 600;
pub(crate) const SETTLE_MARGIN: TimeDelta = TimeDelta::minutes(15);
const SETTLING: TimeDelta = TimeDelta::hours(24);
const MAX_LAG_SESSIONS: usize = 20;
const REFUSED_REQUEST: [u16; 2] = [400, 422];
const DORMANT_RETRY: TimeDelta = TimeDelta::days(7);
const INDEX_RANKS: usize = CORE_ETFS.len() + 1;
const ALONE_PER_RUN: usize = 20;
const RECOVERED_PER_RUN: usize = 20;
const RECOVERY_RETRY: TimeDelta = TimeDelta::days(30);
const HALT_SESSIONS: i64 = 60;

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
                    tickers: Tickers::Today,
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
                            if !page.quiet.is_empty() {
                                self.market
                                    .record_quiet_sessions(DataSource::Alpaca, page.quiet)
                                    .await?;
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
        if requests + 2 <= REQUESTS_PER_RUN
            && !self.stopping()
            && self
                .rate_limited_until(now, DataSource::Alpaca)
                .await?
                .is_none()
        {
            let budget = (REQUESTS_PER_RUN - requests) / 2;
            self.recover_listings(
                now,
                alpaca,
                &calendar,
                &target,
                budget.min(RECOVERED_PER_RUN),
                &mut batch,
            )
            .await?;
        }
        let stored: BTreeMap<Symbol, Instrument> = self
            .market
            .instruments()
            .await?
            .into_iter()
            .map(|instrument| (instrument.symbol.clone(), instrument))
            .collect();
        self.settle_listing_ends(stored.values().cloned().collect(), &stored)
            .await?;
        self.end_silent_listings(floor, &target, &mut batch).await?;
        Ok(batch.finish("daily bars"))
    }

    async fn end_silent_listings(
        &self,
        floor: NaiveDate,
        target: &Session,
        batch: &mut Batch,
    ) -> Result<(), IngestError> {
        let silences = self.market.long_silences(HALT_SESSIONS).await?;
        if silences.is_empty() {
            return Ok(());
        }
        let mut instruments = Vec::new();
        let mut listings = Vec::new();
        for (ticker, last) in silences {
            let ended = Symbol::ended(&ticker, last);
            self.market
                .end_quiet_sessions(DataSource::Alpaca, ticker.clone(), ended.clone(), last)
                .await?;
            listings.extend(
                self.market
                    .bars_between(
                        Timeframe::Day,
                        vec![ticker],
                        day_start(floor),
                        day_start(last + TimeDelta::days(1)),
                    )
                    .await?
                    .into_iter()
                    .map(|bar| Bar {
                        symbol: ended.clone(),
                        ..bar
                    }),
            );
            instruments.push(Instrument {
                symbol: ended,
                name: String::new(),
                exchange: String::new(),
                asset_class: AssetClass::Stock,
                active: false,
                shortable: false,
                first_session: None,
                last_session: Some(last),
            });
        }
        self.market
            .upsert_instruments(DataSource::Alpaca, instruments)
            .await?;
        let rows = self
            .market
            .upsert_bars(DataSource::Alpaca, Timeframe::Day, listings)
            .await?
            .rows;
        batch.success(rows);
        let withdrawn = self
            .market
            .withdraw_shadowed_bars(DataSource::Alpaca, floor, target.date)
            .await?;
        batch.success(withdrawn.rows);
        Ok(())
    }

    async fn recover_listings(
        &self,
        now: Timestamp,
        alpaca: &AlpacaClient,
        calendar: &Calendar,
        target: &Session,
        budget: usize,
        batch: &mut Batch,
    ) -> Result<(), IngestError> {
        let floor = self.config.history_start.max(HISTORY_START);
        let gaps = self
            .market
            .unpriced_members(StockIndex::Sp500, floor, target.date)
            .await?;
        let mut attempted = 0;
        let mut recovered = 0;
        for gap in gaps {
            let tried = self
                .state
                .recent_fetches(
                    DataSource::Alpaca,
                    Dataset::Bars1d,
                    Some(gap.symbol.clone()),
                    10,
                )
                .await?
                .iter()
                .any(|fetch| {
                    (fetch.requested_from, fetch.requested_to) == (Some(gap.from), Some(gap.to))
                        && read(fetch)
                        && fetch.at + RECOVERY_RETRY > now
                });
            if tried {
                continue;
            }
            if attempted == budget || self.stopping() {
                break;
            }
            attempted += 1;
            let key = FetchKey::new(
                DataSource::Alpaca,
                Dataset::Bars1d,
                Some(&gap.symbol),
                Some((gap.from, gap.to)),
            );
            let stored = |symbol: Symbol| async move {
                let bars = self
                    .market
                    .bars_between(Timeframe::Day, vec![symbol], day_start(floor), target.close)
                    .await?;
                Ok::<_, IngestError>(
                    bars.into_iter()
                        .map(|bar| (bar.start, bar))
                        .collect::<BTreeMap<Timestamp, Bar>>(),
                )
            };
            let listed = self
                .market
                .listings()
                .listing(&gap.symbol, gap.from)
                .unwrap_or_else(|| gap.symbol.clone());
            let held = stored(listed.clone()).await?;
            let around = Around {
                gap: &gap,
                listed: &listed,
                held: &held,
            };
            let found = match around.ask(alpaca, calendar, floor, target).await {
                Ok(found) => found,
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    if batch.source_error(gap.symbol.as_str(), &error) {
                        break;
                    }
                    continue;
                }
            };
            let (stored_as, bars) = match found {
                Found::Nothing => {
                    self.record_fetched(now, &key, 0).await?;
                    continue;
                }
                Found::Former(bars) => (listed, bars),
                Found::Listing(listing) => {
                    let final_bar = listing.last().cloned().expect("a found listing has bars");
                    let last = final_bar.start.exchange_date();
                    let still_listed = last >= target.date
                        || stored(gap.symbol.clone()).await?.last_key_value()
                            == Some((&final_bar.start, &final_bar));
                    let stored_as = if listed != gap.symbol || still_listed {
                        listed
                    } else {
                        let ended = Symbol::ended(&gap.symbol, last);
                        self.market
                            .upsert_instruments(
                                DataSource::Alpaca,
                                vec![Instrument {
                                    symbol: ended.clone(),
                                    name: String::new(),
                                    exchange: String::new(),
                                    asset_class: AssetClass::Stock,
                                    active: false,
                                    shortable: false,
                                    first_session: None,
                                    last_session: Some(last),
                                }],
                            )
                            .await?;
                        ended
                    };
                    (stored_as, listing)
                }
            };
            let present = stored(stored_as.clone()).await?;
            let missing: Vec<Bar> = bars
                .into_iter()
                .filter(|bar| !present.contains_key(&bar.start))
                .map(|bar| Bar {
                    symbol: stored_as.clone(),
                    ..bar
                })
                .collect();
            let count = missing.len();
            if count > 0 {
                let rows = self
                    .market
                    .upsert_bars(DataSource::Alpaca, Timeframe::Day, missing)
                    .await?
                    .rows;
                batch.success(rows);
                recovered += 1;
            }
            self.record_fetched(now, &key, count).await?;
        }
        if recovered > 0 {
            let withdrawn = self
                .market
                .withdraw_shadowed_bars(DataSource::Alpaca, floor, target.date)
                .await?;
            batch.success(withdrawn.rows);
        }
        Ok(())
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

enum Found {
    Nothing,
    Listing(Vec<Bar>),
    Former(Vec<Bar>),
}

struct Around<'a> {
    gap: &'a CoverageGap,
    listed: &'a Symbol,
    held: &'a BTreeMap<Timestamp, Bar>,
}

impl Around<'_> {
    fn as_listed(&self, bar: Bar) -> Bar {
        Bar {
            symbol: self.listed.clone(),
            ..bar
        }
    }

    fn in_gap(&self, bar: &Bar) -> bool {
        (self.gap.from..=self.gap.to).contains(&bar.start.exchange_date())
    }

    fn ends_the_held(&self, listing: &[Bar]) -> bool {
        let Some(last) = listing.last() else {
            return false;
        };
        last.start.exchange_date() < self.gap.from
            && self.held.range(..=last.start).all(|(start, bar)| {
                listing
                    .binary_search_by_key(start, |listed| listed.start)
                    .is_ok_and(|index| self.as_listed(listing[index].clone()) == *bar)
            })
    }

    async fn ask(
        &self,
        alpaca: &AlpacaClient,
        calendar: &Calendar,
        floor: NaiveDate,
        target: &Session,
    ) -> Result<Found, SourceError> {
        let ask =
            |tickers| listing_bars(alpaca, calendar, &self.gap.symbol, floor, target, tickers);
        let traded: BTreeMap<Timestamp, Bar> = ask(Tickers::AsTraded)
            .await?
            .into_iter()
            .map(|bar| (bar.start, bar))
            .collect();
        let before = self
            .held
            .range(..day_start(self.gap.from))
            .next_back()
            .map(|(start, _)| start.exchange_date());
        for anchor in [Some(self.gap.to), before].into_iter().flatten() {
            let listing: Vec<Bar> = ask(Tickers::On(anchor))
                .await?
                .into_iter()
                .filter(|bar| traded.get(&bar.start) == Some(bar))
                .collect();
            if listing.iter().any(|bar| self.in_gap(bar)) || self.ends_the_held(&listing) {
                return Ok(Found::Listing(listing));
            }
        }
        let Some(after) = self
            .held
            .range(day_start(self.gap.to + TimeDelta::days(1))..)
            .next()
            .map(|(_, bar)| bar)
        else {
            return Ok(Found::Nothing);
        };
        let mapped: Vec<Bar> = ask(Tickers::On(after.start.exchange_date()))
            .await?
            .into_iter()
            .map(|bar| self.as_listed(bar))
            .collect();
        if !mapped.contains(after) {
            return Ok(Found::Nothing);
        }
        let former: Vec<Bar> = mapped.into_iter().filter(|bar| self.in_gap(bar)).collect();
        Ok(if former.is_empty() {
            Found::Nothing
        } else {
            Found::Former(former)
        })
    }
}

async fn listing_bars(
    alpaca: &AlpacaClient,
    calendar: &Calendar,
    ticker: &Symbol,
    from: NaiveDate,
    target: &Session,
    tickers: Tickers,
) -> Result<Vec<Bar>, SourceError> {
    let request = BarsRequest {
        symbols: vec![ticker.clone()],
        timeframe: Timeframe::Day,
        from: day_start(from),
        to: target.close,
        feed: Feed::Sip,
        tickers,
    };
    let mut bars = Vec::new();
    let mut token = None;
    loop {
        let page = alpaca.bars_page(&request, calendar, token.take()).await?;
        bars.extend(page.bars);
        match page.next {
            Some(next) => token = Some(next),
            None => return Ok(bars),
        }
    }
}
