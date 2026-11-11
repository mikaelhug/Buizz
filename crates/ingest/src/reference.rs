use std::collections::{BTreeMap, HashSet};

use buizz_core::market::{DataSource, Dataset, Instrument, RiskFreeRate, Session, Symbol};
use buizz_core::time::Timestamp;
use chrono::{Datelike, NaiveDate, TimeDelta};

use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError};

pub(crate) const CALENDAR_HORIZON_DAYS: i64 = 365;

impl Ingest {
    pub(crate) async fn calendar(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(alpaca) = &self.sources.alpaca else {
            return Ok(Work::unconfigured("Alpaca"));
        };
        let from = self.config.history_start;
        let to = now.exchange_date() + TimeDelta::days(CALENDAR_HORIZON_DAYS);
        let key = FetchKey::new(
            DataSource::Alpaca,
            Dataset::Calendar,
            None,
            Some((from, to)),
        );
        if let Some(work) = self
            .backoff_work(now, DataSource::Alpaca)
            .await?
        {
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
        if let Some(work) = self
            .backoff_work(now, DataSource::Alpaca)
            .await?
        {
            return Ok(work);
        }
        let assets = match alpaca.assets().await {
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
        let changed: Vec<Instrument> = assets
            .into_iter()
            .filter(|asset| stored.get(&asset.symbol) != Some(asset))
            .collect();
        if changed.is_empty() {
            return Ok(Work::noop("instruments unchanged"));
        }
        let summary = self
            .market
            .upsert_instruments(DataSource::Alpaca, changed)
            .await?;
        Ok(Work::ok(
            summary.rows,
            format!("instruments: {} new or changed", summary.rows),
        ))
    }

    pub(crate) async fn rates(&self, now: Timestamp) -> Result<Work, IngestError> {
        if let Some(work) = self
            .backoff_work(now, DataSource::Treasury)
            .await?
        {
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
        Ok(batch.finish("rates"))
    }
}
