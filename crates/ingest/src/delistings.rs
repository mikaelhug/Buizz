use std::collections::BTreeMap;

use buizz_core::market::{
    AssetClass, Bar, Calendar, DataSource, Dataset, DelistingCheck, DelistingOutcome, Instrument,
    PendingDelisting, SecFiling, Session, Symbol, Timeframe,
};
use buizz_core::time::Timestamp;
use buizz_sources::alpaca::{AlpacaClient, HISTORY_START, Tickers};
use buizz_sources::edgar::EdgarClient;
use buizz_sources::http::SourceError;
use chrono::{Datelike, NaiveDate, TimeDelta};

use crate::bars::listing_bars;
use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError};

const FILERS_PER_RUN: usize = 300;
const DELISTINGS_PER_RUN: usize = 100;
const LAST_TRADE_BEFORE: TimeDelta = TimeDelta::days(180);
const LAST_TRADE_AFTER: TimeDelta = TimeDelta::days(30);

pub(crate) fn quarter_of(date: NaiveDate) -> (i32, u32) {
    (date.year(), date.month0() / 3 + 1)
}

pub(crate) fn next_quarter(quarter: (i32, u32)) -> (i32, u32) {
    quarter_of(quarter_span(quarter).1 + TimeDelta::days(1))
}

pub(crate) fn quarter_span((year, quarter): (i32, u32)) -> (NaiveDate, NaiveDate) {
    let first =
        NaiveDate::from_ymd_opt(year, quarter * 3 - 2, 1).expect("a quarter starts on a date");
    let next = first
        .checked_add_months(chrono::Months::new(3))
        .expect("the next quarter is a date");
    (first, next - TimeDelta::days(1))
}

fn notice_checks(notice: &SecFiling, tickers: &[Symbol]) -> Vec<DelistingCheck> {
    if tickers.is_empty() {
        return vec![DelistingCheck {
            accession: notice.accession.clone(),
            ticker: None,
            listing: None,
            outcome: DelistingOutcome::NoTicker,
        }];
    }
    tickers
        .iter()
        .map(|ticker| DelistingCheck {
            accession: notice.accession.clone(),
            ticker: Some(ticker.clone()),
            listing: None,
            outcome: DelistingOutcome::Pending,
        })
        .collect()
}

impl Ingest {
    pub(crate) async fn delistings(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(edgar) = &self.sources.edgar else {
            return Ok(Work::unconfigured("SEC EDGAR"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Edgar).await? {
            return Ok(work);
        }
        let floor = self.config.history_start.max(HISTORY_START);
        let mut batch = Batch::default();
        if self.read_quarters(now, edgar, floor, &mut batch).await? {
            self.resolve_tickers(now, edgar, floor, &mut batch).await?;
        }
        Ok(batch.finish("delistings"))
    }

    async fn read_quarters(
        &self,
        now: Timestamp,
        edgar: &EdgarClient,
        floor: NaiveDate,
        batch: &mut Batch,
    ) -> Result<bool, IngestError> {
        let read_through = self
            .market
            .coverage()
            .await?
            .into_iter()
            .find(|row| row.dataset == Dataset::Delistings && row.symbol.is_none())
            .map(|row| row.last);
        let current = quarter_of(now.exchange_date());
        let mut quarter = quarter_of(read_through.map_or(floor, |last| last + TimeDelta::days(1)));
        while quarter < current {
            if self.stopping() {
                batch.more_remains("quarters");
                return Ok(false);
            }
            let span = quarter_span(quarter);
            let key = FetchKey::new(DataSource::Edgar, Dataset::Delistings, None, Some(span));
            match edgar.delisting_notices(quarter.0, quarter.1).await {
                Ok(notices) => {
                    let fetched = notices.len();
                    let summary = self
                        .market
                        .append_delisting_notices(DataSource::Edgar, notices, span)
                        .await?;
                    self.record_fetched(now, &key, fetched).await?;
                    batch.success(summary.rows);
                }
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    batch.source_error(&format!("{}Q{}", quarter.0, quarter.1), &error);
                    return Ok(false);
                }
            }
            quarter = next_quarter(quarter);
        }
        Ok(true)
    }

    async fn resolve_tickers(
        &self,
        now: Timestamp,
        edgar: &EdgarClient,
        floor: NaiveDate,
        batch: &mut Batch,
    ) -> Result<(), IngestError> {
        let mut filers: Vec<(u32, Vec<SecFiling>)> = Vec::new();
        let mut positions: BTreeMap<u32, usize> = BTreeMap::new();
        for notice in self.market.unresolved_delistings(floor).await? {
            let position = *positions.entry(notice.cik).or_insert_with(|| {
                filers.push((notice.cik, Vec::new()));
                filers.len() - 1
            });
            filers[position].1.push(notice);
        }
        if filers.len() > FILERS_PER_RUN {
            batch.more_remains("filers");
        }
        for (cik, notices) in filers.into_iter().take(FILERS_PER_RUN) {
            if self.stopping() {
                batch.more_remains("filers");
                break;
            }
            let key = FetchKey::new(DataSource::Edgar, Dataset::Delistings, None, None);
            let checks = match self.filer_checks(edgar, cik, &notices).await {
                Ok(checks) => checks,
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    if batch.source_error(&format!("CIK {cik}"), &error) {
                        break;
                    }
                    continue;
                }
            };
            let summary = self
                .market
                .record_delistings(DataSource::Edgar, checks)
                .await?;
            batch.success(summary.rows);
        }
        Ok(())
    }

    async fn filer_checks(
        &self,
        edgar: &EdgarClient,
        cik: u32,
        notices: &[SecFiling],
    ) -> Result<Vec<DelistingCheck>, SourceError> {
        let submissions = match edgar.submissions(&Symbol::new(&cik.to_string()), cik).await {
            Ok(submissions) => submissions,
            Err(SourceError::Status { status: 404, .. }) => {
                return Ok(notices
                    .iter()
                    .flat_map(|notice| notice_checks(notice, &[]))
                    .collect());
            }
            Err(error) => return Err(error),
        };
        let mut covers: BTreeMap<String, Vec<Symbol>> = BTreeMap::new();
        let mut checks = Vec::new();
        for notice in notices {
            let report = submissions
                .filings
                .periodic
                .iter()
                .filter(|report| report.filed <= notice.filed)
                .max_by_key(|report| report.filed);
            let mut tickers = match report {
                Some(report) => match covers.get(&report.accession) {
                    Some(tickers) => tickers.clone(),
                    None => {
                        let tickers = edgar.report_tickers(cik, &report.accession).await?;
                        covers.insert(report.accession.clone(), tickers.clone());
                        tickers
                    }
                },
                None => Vec::new(),
            };
            if tickers.is_empty() {
                tickers.clone_from(&submissions.tickers);
            }
            checks.extend(notice_checks(notice, &tickers));
        }
        Ok(checks)
    }

    async fn stored_listing(
        &self,
        ticker: &Symbol,
        last: &Bar,
    ) -> Result<Option<Symbol>, IngestError> {
        let Some(listing) = self
            .market
            .listings()
            .listing(ticker, last.start.exchange_date())
        else {
            return Ok(None);
        };
        let held = self
            .market
            .bars_between(
                Timeframe::Day,
                vec![listing.clone()],
                last.start,
                last.start + TimeDelta::days(1),
            )
            .await?;
        Ok((!held.is_empty()).then_some(listing))
    }

    pub(crate) async fn price_delistings(
        &self,
        now: Timestamp,
        alpaca: &AlpacaClient,
        calendar: &Calendar,
        target: &Session,
        batch: &mut Batch,
    ) -> Result<(), IngestError> {
        let floor = self.config.history_start.max(HISTORY_START);
        let pending = self.market.pending_delistings().await?;
        if pending.len() > DELISTINGS_PER_RUN {
            batch.more_remains("delistings");
        }
        let mut stored = false;
        for delisting in pending.into_iter().take(DELISTINGS_PER_RUN) {
            if self.stopping() {
                batch.more_remains("delistings");
                break;
            }
            let key = FetchKey::new(
                DataSource::Alpaca,
                Dataset::Delistings,
                Some(&delisting.ticker),
                Some((floor, delisting.filed)),
            );
            let listing = match delisted_listing(alpaca, calendar, floor, target, &delisting).await
            {
                Ok(listing) => listing,
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    if batch.source_error(delisting.ticker.as_str(), &error) {
                        break;
                    }
                    continue;
                }
            };
            self.record_fetched(now, &key, listing.len()).await?;
            let (outcome, listed) = match listing.last() {
                None => (DelistingOutcome::NoBars, None),
                Some(last) => match self.stored_listing(&delisting.ticker, last).await? {
                    Some(held) => (DelistingOutcome::Held, Some(held)),
                    None => {
                        let last_session = last.start.exchange_date();
                        let ended = Symbol::ended(&delisting.ticker, last_session);
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
                                    last_session: Some(last_session),
                                }],
                            )
                            .await?;
                        let bars = listing
                            .into_iter()
                            .map(|bar| Bar {
                                symbol: ended.clone(),
                                ..bar
                            })
                            .collect();
                        let rows = self
                            .market
                            .upsert_bars(DataSource::Alpaca, Timeframe::Day, bars)
                            .await?
                            .rows;
                        batch.success(rows);
                        stored = true;
                        (DelistingOutcome::Stored, Some(ended))
                    }
                },
            };
            let checks = delisting
                .accessions
                .iter()
                .map(|accession| DelistingCheck {
                    accession: accession.clone(),
                    ticker: Some(delisting.ticker.clone()),
                    listing: listed.clone(),
                    outcome,
                })
                .collect();
            let summary = self
                .market
                .record_delistings(DataSource::Alpaca, checks)
                .await?;
            batch.success(summary.rows);
        }
        if stored {
            let withdrawn = self
                .market
                .withdraw_shadowed_bars(DataSource::Alpaca, floor, target.date)
                .await?;
            batch.success(withdrawn.rows);
        }
        Ok(())
    }
}

async fn delisted_listing(
    alpaca: &AlpacaClient,
    calendar: &Calendar,
    floor: NaiveDate,
    target: &Session,
    delisting: &PendingDelisting,
) -> Result<Vec<Bar>, SourceError> {
    let ask = |tickers| listing_bars(alpaca, calendar, &delisting.ticker, floor, target, tickers);
    let mapped = ask(Tickers::On(delisting.filed)).await?;
    let Some(last) = mapped.last().map(|bar| bar.start.exchange_date()) else {
        return Ok(Vec::new());
    };
    if last < delisting.filed - LAST_TRADE_BEFORE || last > delisting.filed + LAST_TRADE_AFTER {
        return Ok(Vec::new());
    }
    let traded: BTreeMap<Timestamp, Bar> = ask(Tickers::AsTraded)
        .await?
        .into_iter()
        .map(|bar| (bar.start, bar))
        .collect();
    Ok(mapped
        .into_iter()
        .filter(|bar| traded.get(&bar.start) == Some(bar))
        .collect())
}
