use std::collections::HashSet;

use buizz_core::market::{CorporateAction, DataSource, Dataset, Symbol};
use buizz_core::time::Timestamp;
use buizz_market::ActionWindow;
use buizz_sources::alpaca::ACTIONS_SYMBOLS_PER_REQUEST;
use chrono::{NaiveDate, TimeDelta};

use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError};

const PROCESS_DATE_SLACK: TimeDelta = TimeDelta::days(30);
const REFRESH_WINDOW: TimeDelta = TimeDelta::days(90);
const ANNOUNCED_HORIZON: TimeDelta = TimeDelta::days(90);
const BACKFILL_REQUESTS_PER_RUN: usize = 40;

impl Ingest {
    pub(crate) async fn corporate_actions(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(alpaca) = &self.sources.alpaca else {
            return Ok(Work::unconfigured("Alpaca"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Alpaca).await? {
            return Ok(work);
        }
        let symbols: Vec<Symbol> = self
            .priced_symbols()
            .await?
            .into_iter()
            .map(|priced| priced.symbol)
            .collect();
        if symbols.is_empty() {
            return Ok(Work::noop("no instruments to price"));
        }
        let today = now.exchange_date();
        let through = today + ANNOUNCED_HORIZON;
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
        let refresh_from = (today - REFRESH_WINDOW).max(history_start);
        let mut batch = Batch::default();
        let mut requests: Vec<(NaiveDate, Vec<Symbol>, bool)> = Vec::new();
        if !refresh.is_empty() {
            requests.push((refresh_from, refresh, false));
        }
        let backfills = backfill.chunks(ACTIONS_SYMBOLS_PER_REQUEST);
        requests.extend(backfills.map(|chunk| (history_start, chunk.to_vec(), true)));
        let budget = BACKFILL_REQUESTS_PER_RUN
            + usize::from(
                !requests
                    .first()
                    .is_some_and(|(_, _, backfilling)| *backfilling),
            );
        for (index, (from, scope, backfilling)) in requests.into_iter().enumerate() {
            if index == budget || self.stopping() {
                batch.more_remains("symbols");
                break;
            }
            let key = FetchKey::new(
                DataSource::Alpaca,
                Dataset::CorporateActions,
                None,
                Some((from, through)),
            );
            let (start, end) = (from - PROCESS_DATE_SLACK, through + PROCESS_DATE_SLACK);
            let fetched = if backfilling {
                alpaca.corporate_actions(&scope, start, end).await
            } else {
                alpaca.market_corporate_actions(start, end).await
            };
            let fetched = match fetched {
                Ok(actions) => actions,
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    let label = format!("{} symbols from {from}", scope.len());
                    if batch.source_error(&label, &error) {
                        break;
                    }
                    continue;
                }
            };
            let members: HashSet<&Symbol> = scope.iter().collect();
            let fetched: Vec<CorporateAction> = fetched
                .into_iter()
                .filter(|action| {
                    (from..=through).contains(&action.ex_date) && members.contains(&action.symbol)
                })
                .collect();
            let count = fetched.len();
            let summary = self
                .market
                .replace_corporate_actions(
                    DataSource::Alpaca,
                    fetched,
                    ActionWindow {
                        symbols: scope,
                        from,
                        through,
                        covered_to: backfilling.then_some(today),
                    },
                )
                .await?;
            self.record_fetched(now, &key, count).await?;
            batch.success(summary.rows);
        }
        Ok(batch.finish("corporate actions"))
    }
}
