use std::collections::{BTreeMap, BTreeSet, HashSet};

use buizz_core::market::{
    AssetClass, DataSource, Dataset, Instrument, RiskFreeRate, Session, StockIndex, Symbol,
};
use buizz_core::ops::JobOutcome;
use buizz_core::time::Timestamp;
use chrono::{Datelike, NaiveDate, TimeDelta};

use crate::bars::HALT_SESSIONS;
use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError};

pub(crate) const CALENDAR_HORIZON_DAYS: i64 = 365;
const CALENDAR_START_SLACK: TimeDelta = TimeDelta::days(10);
const CALENDAR_RECHECK: TimeDelta = TimeDelta::days(30);

impl Ingest {
    pub(crate) async fn calendar(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(alpaca) = &self.sources.alpaca else {
            return Ok(Work::unconfigured("Alpaca"));
        };
        let history_start = self.config.history_start;
        let backfilled = !self
            .market
            .calendar(history_start, history_start + CALENDAR_START_SLACK)
            .await?
            .sessions()
            .is_empty();
        let from = if backfilled {
            history_start.max(now.exchange_date() - CALENDAR_RECHECK)
        } else {
            history_start
        };
        let to = now.exchange_date() + TimeDelta::days(CALENDAR_HORIZON_DAYS);
        let key = FetchKey::new(
            DataSource::Alpaca,
            Dataset::Calendar,
            None,
            Some((from, to)),
        );
        if let Some(work) = self.backoff_work(now, DataSource::Alpaca).await? {
            return Ok(work);
        }
        let sessions = match alpaca.calendar(from, to).await {
            Ok(sessions) => sessions,
            Err(error) => return self.source_failure(now, &key, &error).await,
        };
        self.record_fetched(now, &key, sessions.len()).await?;
        let stored: HashSet<Session> = self
            .market
            .calendar(from, to)
            .await?
            .sessions()
            .iter()
            .copied()
            .collect();
        let fetched: HashSet<Session> = sessions.iter().copied().collect();
        let added = fetched.difference(&stored).count();
        let removed = stored.difference(&fetched).count();
        if added == 0 && removed == 0 {
            return Ok(Work::noop("calendar unchanged"));
        }
        let summary = self
            .market
            .replace_sessions(DataSource::Alpaca, from, to, sessions)
            .await?;
        Ok(Work::ok(
            summary.rows,
            format!("calendar: {added} sessions new or changed, {removed} removed"),
        ))
    }

    pub(crate) async fn instruments(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(alpaca) = &self.sources.alpaca else {
            return Ok(Work::unconfigured("Alpaca"));
        };
        let key = FetchKey::new(DataSource::Alpaca, Dataset::Instruments, None, None);
        if let Some(work) = self.backoff_work(now, DataSource::Alpaca).await? {
            return Ok(work);
        }
        let mut assets = match alpaca.assets().await {
            Ok(assets) => assets,
            Err(error) => return self.source_failure(now, &key, &error).await,
        };
        self.record_fetched(now, &key, assets.len()).await?;
        let stored: BTreeMap<Symbol, Instrument> = self
            .market
            .instruments()
            .await?
            .into_iter()
            .map(|instrument| (instrument.symbol.clone(), instrument))
            .collect();
        let mut batch = Batch::default();
        let mut funds: Option<BTreeSet<Symbol>> = None;
        let mut fund_lists_complete = true;
        if let Some(edgar) = &self.sources.edgar {
            let key = FetchKey::new(DataSource::Edgar, Dataset::Instruments, None, None);
            match edgar.fund_tickers().await {
                Ok(listed) => {
                    self.record_fetched(now, &key, listed.len()).await?;
                    funds.get_or_insert_default().extend(listed);
                }
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    batch.failure("SEC fund tickers", &error);
                    fund_lists_complete = false;
                }
            }
        }
        if let Some(nasdaq) = &self.sources.nasdaq {
            let key = FetchKey::new(DataSource::Nasdaq, Dataset::Instruments, None, None);
            match nasdaq.etf_symbols().await {
                Ok(listed) => {
                    self.record_fetched(now, &key, listed.len()).await?;
                    funds.get_or_insert_default().extend(listed);
                }
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    batch.failure("NASDAQ symbol directory", &error);
                    fund_lists_complete = false;
                }
            }
        }
        let funds = funds.filter(|_| fund_lists_complete);
        for asset in &mut assets {
            asset.asset_class = match &funds {
                Some(funds) if funds.contains(&asset.symbol) => AssetClass::Etf,
                Some(_) => AssetClass::Stock,
                None => stored
                    .get(&asset.symbol)
                    .map_or(AssetClass::Stock, |known| known.asset_class),
            };
        }
        let rows = self.settle_listing_ends(assets, &stored).await?;
        batch.success(rows);
        Ok(batch.finish("instruments"))
    }

    pub(crate) async fn settle_listing_ends(
        &self,
        mut assets: Vec<Instrument>,
        stored: &BTreeMap<Symbol, Instrument>,
    ) -> Result<u64, IngestError> {
        let priced = self
            .symbol_coverage(Dataset::Bars1d, DataSource::Alpaca)
            .await?;
        let latest = priced.values().map(|coverage| coverage.last).max();
        let ended = |symbol: &Symbol| {
            priced
                .get(symbol)
                .map(|coverage| coverage.last)
                .filter(|last| Some(*last) < latest)
        };
        let silent_before = match latest {
            Some(latest) => self
                .market
                .calendar(self.config.history_start, latest)
                .await?
                .sessions()
                .iter()
                .rev()
                .nth(HALT_SESSIONS as usize)
                .map(|session| session.date),
            None => None,
        };
        for asset in &mut assets {
            if asset.active {
                asset.last_session = priced
                    .get(&asset.symbol)
                    .map(|coverage| coverage.last)
                    .filter(|last| silent_before.is_some_and(|before| *last < before));
            } else {
                asset.last_session = asset.last_session.or_else(|| ended(&asset.symbol));
            }
        }
        let listed: BTreeSet<Symbol> = assets.iter().map(|asset| asset.symbol.clone()).collect();
        let unlisted_members: BTreeSet<Symbol> = self
            .market
            .index_history(StockIndex::Sp500)
            .await?
            .map(|history| history.members)
            .unwrap_or_default()
            .into_iter()
            .map(|member| member.symbol)
            .chain(self.delivered_symbols().await?)
            .filter(|symbol| !listed.contains(symbol) && priced.contains_key(symbol))
            .collect();
        assets.extend(unlisted_members.into_iter().map(|symbol| Instrument {
            name: String::new(),
            exchange: String::new(),
            asset_class: AssetClass::Stock,
            active: false,
            shortable: false,
            first_session: None,
            last_session: ended(&symbol),
            symbol,
        }));
        let changed: Vec<Instrument> = assets
            .into_iter()
            .filter(|asset| stored.get(&asset.symbol) != Some(asset))
            .collect();
        Ok(if changed.is_empty() {
            0
        } else {
            self.market
                .upsert_instruments(DataSource::Alpaca, changed)
                .await?
                .rows
        })
    }

    pub(crate) async fn rates(&self, now: Timestamp) -> Result<Work, IngestError> {
        if let Some(work) = self.backoff_work(now, DataSource::Treasury).await? {
            return Ok(work);
        }
        let treasury = &self.sources.treasury;
        let this_year = now.exchange_date().year();
        let covered = self.market.coverage().await?.into_iter().find(|row| {
            row.dataset == Dataset::Rates
                && row.source == DataSource::Treasury
                && row.symbol.is_none()
        });
        let first_year = covered.map_or(self.config.history_start.year(), |row| {
            row.last.year().min(this_year)
        });
        let mut batch = Batch::default();
        let mut fetched: Vec<RiskFreeRate> = Vec::new();
        for year in first_year..=this_year {
            let key = FetchKey::new(DataSource::Treasury, Dataset::Rates, None, None);
            match treasury.three_month_yields(year).await {
                Ok(rates) => {
                    self.record_fetched(now, &key, rates.len()).await?;
                    batch.success(0);
                    fetched.extend(rates);
                }
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    if batch.source_error(&year.to_string(), &error) {
                        break;
                    }
                }
            }
        }
        let (Some(from), Some(to)) = (
            fetched.iter().map(|rate| rate.date).min(),
            fetched.iter().map(|rate| rate.date).max(),
        ) else {
            return Ok(batch.finish("rates"));
        };
        let stored: BTreeMap<NaiveDate, RiskFreeRate> = self
            .market
            .risk_free_rates(from, to)
            .await?
            .into_iter()
            .map(|rate| (rate.date, rate))
            .collect();
        let changed: Vec<RiskFreeRate> = fetched
            .into_iter()
            .filter(|rate| stored.get(&rate.date) != Some(rate))
            .collect();
        if !changed.is_empty() {
            let summary = self
                .market
                .upsert_risk_free_rates(DataSource::Treasury, changed)
                .await?;
            batch.success(summary.rows);
        }
        let closed = self
            .market
            .calendar(to, now.exchange_date())
            .await?
            .sessions()
            .iter()
            .rfind(|session| session.close <= now)
            .map(|session| session.date);
        let work = batch.finish("rates");
        Ok(match closed {
            Some(closed) if work.outcome == JobOutcome::Noop && to < closed => {
                Work::waiting(format!("rates: {closed} is not published yet"))
            }
            _ => work,
        })
    }
}
