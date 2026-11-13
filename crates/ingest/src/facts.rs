use std::collections::{BTreeMap, BTreeSet};

use buizz_core::market::{AssetClass, DataSource, Dataset, Symbol};
use buizz_core::time::Timestamp;
use buizz_sources::edgar::ACCEPTED_FORMS;
use chrono::{NaiveDate, TimeDelta};

use crate::fetch::{Batch, FetchKey, Work, read, waiting};
use crate::reference::CALENDAR_HORIZON_DAYS;
use crate::{Ingest, IngestError, Priced};

const COMPANIES_PER_RUN: usize = 500;
const SAFETY_REFRESH: TimeDelta = TimeDelta::days(90);
pub(crate) const NO_CIK: &str = "no SEC CIK for this ticker";

impl Ingest {
    pub(crate) async fn fundamentals(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(edgar) = &self.sources.edgar else {
            return Ok(Work::unconfigured("SEC EDGAR"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Edgar).await? {
            return Ok(work);
        }
        let map_key = FetchKey::new(DataSource::Edgar, Dataset::Fundamentals, None, None);
        let tickers = match edgar.ticker_map().await {
            Ok(tickers) => tickers,
            Err(error) => return self.source_failure(now, &map_key, &error).await,
        };
        let (due, more) = self
            .companies_due(
                now,
                Dataset::Fundamentals,
                &tickers,
                &ACCEPTED_FORMS,
                &BTreeSet::new(),
            )
            .await?;
        if due.is_empty() {
            return Ok(Work::noop("no company has a new filing"));
        }
        let from = self.config.history_start;
        let to = now.exchange_date() + TimeDelta::days(CALENDAR_HORIZON_DAYS);
        let calendar = self.market.calendar(from, to).await?;
        let mut batch = Batch::default();
        let mut without_cik = 0;
        let mut more = more;
        for symbol in due {
            if self.stopping() {
                more = true;
                break;
            }
            let key = FetchKey::new(
                DataSource::Edgar,
                Dataset::Fundamentals,
                Some(&symbol),
                None,
            );
            let Some(cik) = tickers.get(&symbol) else {
                self.record_rejection(now, &key, NO_CIK).await?;
                without_cik += 1;
                continue;
            };
            match edgar.company_facts(&symbol, *cik, &calendar).await {
                Ok(facts) => {
                    let fetched = facts.len();
                    let summary = self
                        .market
                        .append_fundamental_facts(DataSource::Edgar, facts)
                        .await?;
                    self.record_fetched(now, &key, fetched).await?;
                    batch.success(summary.rows);
                }
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    if batch.source_error(symbol.as_str(), &error) {
                        break;
                    }
                }
            }
        }
        if more {
            batch.more_remains("companies");
        }
        let mut work = batch.finish("fundamentals");
        if without_cik > 0 {
            work.message
                .push_str(&format!(" ({without_cik} tickers without a CIK)"));
        }
        Ok(work)
    }

    pub(crate) async fn companies_due(
        &self,
        now: Timestamp,
        dataset: Dataset,
        tickers: &BTreeMap<Symbol, u32>,
        forms: &'static [&'static str],
        pending: &BTreeSet<Symbol>,
    ) -> Result<(Vec<Symbol>, bool), IngestError> {
        let stocks: BTreeSet<Symbol> = self
            .market
            .instruments()
            .await?
            .into_iter()
            .filter(|instrument| instrument.active && instrument.asset_class == AssetClass::Stock)
            .map(|instrument| instrument.symbol)
            .collect();
        let filed = self.market.latest_sec_filings(forms).await?;
        let mut due: Vec<Symbol> = Vec::new();
        for Priced { symbol, .. } in self.priced_symbols().await? {
            if !stocks.contains(&symbol) {
                continue;
            }
            if due.len() == COMPANIES_PER_RUN {
                return Ok((due, true));
            }
            let last_filed = tickers.get(&symbol).and_then(|cik| filed.get(cik)).copied();
            let unread = pending.contains(&symbol);
            if self
                .company_due(now, dataset, &symbol, last_filed, unread)
                .await?
            {
                due.push(symbol);
            }
        }
        Ok((due, false))
    }

    async fn company_due(
        &self,
        now: Timestamp,
        dataset: Dataset,
        symbol: &Symbol,
        last_filed: Option<NaiveDate>,
        unread: bool,
    ) -> Result<bool, IngestError> {
        let latest = self
            .latest_fetch(DataSource::Edgar, dataset, symbol)
            .await?;
        Ok(latest.is_none_or(|fetch| {
            let fresh = read(&fetch)
                && !unread
                && fetch.at + SAFETY_REFRESH > now
                && last_filed.is_none_or(|filed| filed < fetch.at.exchange_date());
            !waiting(&fetch, now) && !fresh
        }))
    }
}
