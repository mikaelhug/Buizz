use std::collections::BTreeMap;

use buizz_core::market::{Bar, DataSource, Dataset, Feed, Symbol, Timeframe};
use buizz_core::ops::CoverageGap;
use buizz_core::time::Timestamp;
use buizz_sources::alpaca::{BarsRequest, SIP_DELAY, Tickers};
use buizz_sources::http::SourceError;
use chrono::{NaiveDate, TimeDelta};

use crate::bars::SETTLE_MARGIN;
use crate::fetch::{FetchKey, Work};
use crate::{Ingest, IngestError, day_start};

const SYMBOLS_PER_REQUEST: usize = 50;
const REQUESTS_PER_RUN: usize = 600;

#[derive(Default)]
struct Filled {
    rows: u64,
    left: Vec<CoverageGap>,
    failure: Option<SourceError>,
}

impl Ingest {
    pub async fn ensure_minute_coverage(
        &self,
        symbols: &[Symbol],
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<(), IngestError> {
        let gaps = self
            .market
            .coverage_gaps(symbols.to_vec(), Timeframe::Minute, from, to)
            .await?;
        if gaps.is_empty() {
            return Ok(());
        }
        let now = Timestamp::now();
        if let Some(until) = self.rate_limited_until(now, DataSource::Alpaca).await? {
            return Err(SourceError::RateLimited {
                retry_after: (until - now).to_std().ok(),
            }
            .into());
        }
        match self.fill_minute_gaps(now, gaps, None).await?.failure {
            Some(error) => Err(error.into()),
            None => Ok(()),
        }
    }

    pub(crate) async fn minute_bars(&self, now: Timestamp) -> Result<Work, IngestError> {
        if self.sources.alpaca.is_none() {
            return Ok(Work::unconfigured("Alpaca"));
        }
        if let Some(work) = self.backoff_work(now, DataSource::Alpaca).await? {
            return Ok(work);
        }
        let firsts: BTreeMap<Symbol, NaiveDate> = self
            .symbol_coverage(Dataset::Bars1m, DataSource::Alpaca)
            .await?
            .into_iter()
            .map(|(symbol, coverage)| (symbol, coverage.first))
            .collect();
        let Some(from) = firsts.values().min().copied() else {
            return Ok(Work::noop("minute bars: no symbol has minute bars yet"));
        };
        let settled = now
            - TimeDelta::from_std(SIP_DELAY).expect("the SIP delay fits a time delta")
            - SETTLE_MARGIN;
        let calendar = self.market.calendar(from, now.exchange_date()).await?;
        let Some(to) = calendar
            .sessions()
            .iter()
            .rfind(|session| session.close <= settled)
            .map(|session| session.date)
        else {
            return Ok(Work::waiting(
                "minute bars: no settled session in the calendar",
            ));
        };
        let gaps: Vec<CoverageGap> = self
            .market
            .coverage_gaps(
                firsts.keys().cloned().collect(),
                Timeframe::Minute,
                from,
                to,
            )
            .await?
            .into_iter()
            .filter_map(|gap| {
                let first = *firsts.get(&gap.symbol)?;
                (gap.to >= first).then(|| CoverageGap {
                    from: gap.from.max(first),
                    ..gap
                })
            })
            .collect();
        if gaps.is_empty() {
            return Ok(Work::noop(format!(
                "minute bars: {} symbols filled through {to}",
                firsts.len()
            )));
        }
        let Filled {
            rows,
            left,
            failure,
        } = self
            .fill_minute_gaps(now, gaps, Some(REQUESTS_PER_RUN))
            .await?;
        let progress = match left.iter().map(|gap| gap.from).min() {
            None => format!("{} symbols read through {to}", firsts.len()),
            Some(earliest) => {
                let sessions: usize = left
                    .iter()
                    .map(|gap| calendar.between(gap.from, gap.to).len())
                    .sum();
                format!("{sessions} symbol sessions from {earliest} left")
            }
        };
        let message = format!("minute bars: {rows} rows, {progress}");
        Ok(match failure {
            Some(SourceError::RateLimited { .. }) => Work::backoff(rows, message),
            Some(error) if rows == 0 => Work::failed(format!("{message}: {error}")),
            Some(error) => Work::partial(rows, format!("{message}: {error}")),
            None if !left.is_empty() => Work::partial(rows, message),
            None => Work::ok(rows, message),
        })
    }

    async fn fill_minute_gaps(
        &self,
        now: Timestamp,
        gaps: Vec<CoverageGap>,
        budget: Option<usize>,
    ) -> Result<Filled, IngestError> {
        let alpaca = self
            .sources
            .alpaca
            .as_ref()
            .ok_or(IngestError::Unconfigured("Alpaca"))?;
        let (Some(from), Some(to)) = (
            gaps.iter().map(|gap| gap.from).min(),
            gaps.iter().map(|gap| gap.to).max(),
        ) else {
            return Ok(Filled::default());
        };
        let calendar = self.market.calendar(from, to).await?;
        let listings = self.market.listings();
        let mut grouped: BTreeMap<(NaiveDate, NaiveDate), Vec<Symbol>> = BTreeMap::new();
        for gap in gaps {
            grouped
                .entry((gap.from, gap.to))
                .or_default()
                .push(gap.symbol);
        }
        let out_of_budget =
            |requests: usize| budget.is_some_and(|budget| requests >= budget || self.stopping());
        let mut filled = Filled::default();
        let mut requests = 0;
        for ((gap_from, gap_to), group) in grouped {
            for chunk in group.chunks(SYMBOLS_PER_REQUEST) {
                let keys: BTreeMap<&Symbol, Symbol> = chunk
                    .iter()
                    .filter_map(|ticker| Some((ticker, listings.listing(ticker, gap_from)?)))
                    .collect();
                let request = BarsRequest {
                    symbols: chunk.to_vec(),
                    timeframe: Timeframe::Minute,
                    from: day_start(gap_from),
                    to: day_start(gap_to + TimeDelta::days(1)),
                    feed: Feed::Sip,
                    tickers: Tickers::On(gap_from),
                };
                let key = FetchKey::new(
                    DataSource::Alpaca,
                    Dataset::Bars1m,
                    None,
                    Some((gap_from, gap_to)),
                );
                let mut carried: Vec<Bar> = Vec::new();
                let mut token = None;
                let complete = loop {
                    if filled.failure.is_some() || out_of_budget(requests) {
                        break false;
                    }
                    requests += 1;
                    let page = match alpaca.bars_page(&request, &calendar, token.take()).await {
                        Ok(page) => page,
                        Err(error) => {
                            self.record_failure(now, &key, &error).await?;
                            filled.failure = Some(error);
                            break false;
                        }
                    };
                    let fetched = page.bars.len();
                    let mut bars = std::mem::take(&mut carried);
                    bars.extend(page.bars.into_iter().filter_map(|bar| {
                        let symbol = keys.get(&bar.symbol)?.clone();
                        Some(Bar { symbol, ..bar })
                    }));
                    if page.next.is_some() {
                        carried = split_last_day(&mut bars);
                    }
                    filled.rows += self
                        .market
                        .upsert_bars(DataSource::Alpaca, Timeframe::Minute, bars)
                        .await?
                        .rows;
                    self.record_fetched(now, &key, fetched).await?;
                    match page.next {
                        Some(next) => token = Some(next),
                        None => break true,
                    }
                };
                if !complete {
                    filled.left.extend(chunk.iter().map(|symbol| CoverageGap {
                        symbol: symbol.clone(),
                        timeframe: Timeframe::Minute,
                        from: gap_from,
                        to: gap_to,
                    }));
                }
            }
        }
        Ok(filled)
    }
}

fn split_last_day(bars: &mut Vec<Bar>) -> Vec<Bar> {
    let Some(last) = bars.last() else {
        return Vec::new();
    };
    let (symbol, date) = (last.symbol.clone(), last.start.exchange_date());
    let keep = bars
        .iter()
        .rposition(|bar| bar.symbol != symbol || bar.start.exchange_date() != date)
        .map_or(0, |index| index + 1);
    bars.split_off(keep)
}
