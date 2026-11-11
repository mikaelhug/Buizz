use std::collections::{BTreeMap, HashSet};

use buizz_core::market::{CorporateAction, CorporateActionKind, DataSource, Dataset, Symbol};
use buizz_core::time::Timestamp;
use chrono::{NaiveDate, TimeDelta};

use crate::fetch::{FetchKey, Work};
use crate::{Ingest, IngestError};

const SYMBOLS_PER_REQUEST: usize = 100;
const PROCESS_DATE_SLACK: TimeDelta = TimeDelta::days(30);
const REFRESH_WINDOW: TimeDelta = TimeDelta::days(90);

type ActionKey = (Symbol, NaiveDate, bool);

fn identity(action: &CorporateAction) -> ActionKey {
    (
        action.symbol.clone(),
        action.ex_date,
        matches!(action.kind, CorporateActionKind::Split { .. }),
    )
}

impl Ingest {
    pub(crate) async fn corporate_actions(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(alpaca) = &self.sources.alpaca else {
            return Ok(Work::unconfigured("Alpaca"));
        };
        if let Some(work) = self
            .backoff_work(now, DataSource::Alpaca)
            .await?
        {
            return Ok(work);
        }
        let symbols = self.active_symbols().await?;
        if symbols.is_empty() {
            return Ok(Work::noop("no active instruments"));
        }
        let to = now.exchange_date();
        let history_start = self.config.history_start;
        let backfilled = !self
            .market
            .corporate_actions(symbols.clone(), history_start, to)
            .await?
            .is_empty();
        let from = if backfilled {
            (to - REFRESH_WINDOW).max(history_start)
        } else {
            history_start
        };
        let universe: HashSet<&Symbol> = symbols.iter().collect();
        let mut fetched: BTreeMap<ActionKey, CorporateAction> = BTreeMap::new();
        for chunk in symbols.chunks(SYMBOLS_PER_REQUEST) {
            let key = FetchKey::new(
                DataSource::Alpaca,
                Dataset::CorporateActions,
                None,
                Some((from, to)),
            );
            let request_from = from - PROCESS_DATE_SLACK;
            let request_to = to + PROCESS_DATE_SLACK;
            match alpaca
                .corporate_actions(chunk, request_from, request_to)
                .await
            {
                Ok(actions) => {
                    self.record_fetched(now, &key, actions.len()).await?;
                    for action in actions {
                        let in_range = (from..=to).contains(&action.ex_date);
                        if in_range && universe.contains(&action.symbol) {
                            fetched.insert(identity(&action), action);
                        }
                    }
                }
                Err(error) => return self.source_failure(now, &key, &error).await,
            }
        }
        let stored: BTreeMap<ActionKey, CorporateAction> = self
            .market
            .corporate_actions(symbols, from, to)
            .await?
            .into_iter()
            .map(|action| (identity(&action), action))
            .collect();
        let changed: Vec<CorporateAction> = fetched
            .into_iter()
            .filter(|(key, action)| stored.get(key) != Some(action))
            .map(|(_, action)| action)
            .collect();
        if changed.is_empty() {
            return Ok(Work::noop("corporate actions unchanged"));
        }
        let summary = self
            .market
            .upsert_corporate_actions(DataSource::Alpaca, changed)
            .await?;
        Ok(Work::ok(
            summary.rows,
            format!("corporate actions: {} new or changed", summary.rows),
        ))
    }
}
