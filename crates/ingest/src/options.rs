use std::collections::BTreeSet;

use buizz_core::market::{DataSource, Dataset, StockIndex, Symbol};
use buizz_core::ops::FetchStatus;
use buizz_core::time::Timestamp;
use chrono::TimeDelta;

use crate::fetch::{Batch, FetchKey, Work};
use crate::{CORE_ETFS, Ingest, IngestError};

pub(crate) const READ_BEFORE_CLOSE: TimeDelta = TimeDelta::minutes(30);

impl Ingest {
    pub(crate) async fn option_chains(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(alpaca) = &self.sources.alpaca else {
            return Ok(Work::unconfigured("Alpaca"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Alpaca).await? {
            return Ok(work);
        }
        let today = now.exchange_date();
        let calendar = self.market.calendar(today, today).await?;
        let Some(session) = calendar.session(today).copied() else {
            return Ok(Work::noop("option chains: no session today"));
        };
        let read_at = session.close - READ_BEFORE_CLOSE;
        if now < read_at {
            return Ok(Work::waiting(format!(
                "option chains are read from {read_at}"
            )));
        }
        let underlyings = self.option_underlyings().await?;
        let mut read = self.market.option_underlyings_on(session.date).await?;
        let limit = u32::try_from(underlyings.len() * 2).unwrap_or(u32::MAX);
        read.extend(
            self.state
                .recent_fetches(DataSource::Alpaca, Dataset::OptionChains, None, limit)
                .await?
                .into_iter()
                .filter(|fetch| {
                    fetch.status == FetchStatus::Empty && fetch.requested_from == Some(session.date)
                })
                .filter_map(|fetch| fetch.symbol),
        );
        let pending: Vec<Symbol> = underlyings
            .into_iter()
            .filter(|underlying| !read.contains(underlying))
            .collect();
        if pending.is_empty() {
            return Ok(Work::waiting(format!(
                "option chains of {} already stored",
                session.date
            )));
        }
        let mut batch = Batch::default();
        for underlying in pending {
            if self.stopping() {
                batch.more_remains(format!("option chains from {underlying}"));
                break;
            }
            let key = FetchKey::new(
                DataSource::Alpaca,
                Dataset::OptionChains,
                Some(&underlying),
                Some((session.date, session.date)),
            );
            match alpaca.option_chain(&underlying, session.date).await {
                Ok(quotes) => {
                    self.record_fetched(now, &key, quotes.len()).await?;
                    let summary = self
                        .market
                        .append_option_quotes(DataSource::Alpaca, session.date, quotes)
                        .await?;
                    batch.success(summary.rows);
                }
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    if batch.source_error(underlying.as_str(), &error) {
                        break;
                    }
                }
            }
        }
        Ok(batch.finish("option chains"))
    }

    async fn option_underlyings(&self) -> Result<BTreeSet<Symbol>, IngestError> {
        let members = self
            .market
            .index_history(StockIndex::Sp500)
            .await?
            .map(|history| history.members)
            .unwrap_or_default()
            .into_iter()
            .filter(|member| member.end.is_none())
            .map(|member| member.symbol);
        let listed: BTreeSet<Symbol> = self
            .market
            .instruments()
            .await?
            .into_iter()
            .filter(|instrument| instrument.active)
            .map(|instrument| instrument.symbol)
            .collect();
        Ok(CORE_ETFS
            .iter()
            .map(|etf| Symbol::new(etf))
            .chain(members)
            .filter(|symbol| listed.contains(symbol))
            .filter(|symbol| {
                self.config
                    .only
                    .as_ref()
                    .is_none_or(|only| only.contains(symbol))
            })
            .collect())
    }
}
