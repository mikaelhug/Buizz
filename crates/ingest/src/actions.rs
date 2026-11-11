use std::collections::{BTreeMap, HashSet};

use buizz_core::market::{CorporateAction, CorporateActionKind, DataSource, Dataset, Symbol};
use buizz_core::time::Timestamp;
use chrono::{NaiveDate, TimeDelta};

use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError};

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
        if let Some(work) = self.backoff_work(now, DataSource::Alpaca).await? {
            return Ok(work);
        }
        let symbols = self.priced_symbols().await?;
        if symbols.is_empty() {
            return Ok(Work::noop("no instruments to price"));
        }
        let to = now.exchange_date();
        let history_start = self.config.history_start;
        let covered = self
            .symbol_coverage(Dataset::CorporateActions, DataSource::Alpaca)
            .await?;
        let (refresh, backfill): (Vec<Symbol>, Vec<Symbol>) =
            symbols.into_iter().partition(|symbol| {
                covered
                    .get(symbol)
                    .is_some_and(|coverage| coverage.first <= history_start)
            });
        let refresh_from = (to - REFRESH_WINDOW).max(history_start);
        let mut batch = Batch::default();
        for (from, group, backfilling) in [
            (history_start, backfill, true),
            (refresh_from, refresh, false),
        ] {
            if group.is_empty() {
                continue;
            }
            let key = FetchKey::new(
                DataSource::Alpaca,
                Dataset::CorporateActions,
                None,
                Some((from, to)),
            );
            let fetched = match alpaca
                .corporate_actions(&group, from - PROCESS_DATE_SLACK, to + PROCESS_DATE_SLACK)
                .await
            {
                Ok(actions) => actions,
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    let label = format!("{} symbols from {from}", group.len());
                    if batch.source_error(&label, &error) {
                        break;
                    }
                    continue;
                }
            };
            self.record_fetched(now, &key, fetched.len()).await?;
            let members: HashSet<&Symbol> = group.iter().collect();
            let fetched: BTreeMap<ActionKey, CorporateAction> = fetched
                .into_iter()
                .filter(|action| {
                    (from..=to).contains(&action.ex_date) && members.contains(&action.symbol)
                })
                .map(|action| (identity(&action), action))
                .collect();
            let stored: BTreeMap<ActionKey, CorporateAction> = self
                .market
                .corporate_actions(group.clone(), from, to)
                .await?
                .into_iter()
                .map(|action| (identity(&action), action))
                .collect();
            let changed: Vec<CorporateAction> = fetched
                .into_iter()
                .filter(|(key, action)| stored.get(key) != Some(action))
                .map(|(_, action)| action)
                .collect();
            if changed.is_empty() && !backfilling {
                batch.success(0);
                continue;
            }
            let summary = self
                .market
                .upsert_corporate_actions(DataSource::Alpaca, changed, group, from, to)
                .await?;
            batch.success(summary.rows);
        }
        Ok(batch.finish("corporate actions"))
    }
}
