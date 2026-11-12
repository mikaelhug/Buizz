use std::collections::{BTreeMap, BTreeSet, HashSet};

use buizz_core::market::{CorporateAction, CorporateActionKind, DataSource, Dataset, Symbol};
use buizz_core::time::Timestamp;
use buizz_market::ActionWindow;
use buizz_sources::alpaca::{ACTIONS_SYMBOLS_PER_REQUEST, AlpacaClient};
use chrono::{NaiveDate, TimeDelta};

use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError};

const PROCESS_DATE_SLACK: TimeDelta = TimeDelta::days(30);
const REFRESH_WINDOW: TimeDelta = TimeDelta::days(90);
const ANNOUNCED_HORIZON: TimeDelta = TimeDelta::days(90);
const BACKFILL_REQUESTS_PER_RUN: usize = 40;

#[derive(Default)]
pub(crate) struct Renames(BTreeMap<Symbol, Vec<(NaiveDate, Symbol)>>);

impl Renames {
    pub(crate) fn new<'a>(actions: impl IntoIterator<Item = &'a CorporateAction>) -> Self {
        let mut renames: BTreeMap<Symbol, Vec<(NaiveDate, Symbol)>> = BTreeMap::new();
        for action in actions {
            if let CorporateActionKind::NameChange { new_symbol } = &action.kind
                && *new_symbol != action.symbol
            {
                renames
                    .entry(action.symbol.clone())
                    .or_default()
                    .push((action.ex_date, new_symbol.clone()));
            }
        }
        for changes in renames.values_mut() {
            changes.sort();
        }
        Self(renames)
    }

    pub(crate) fn current(&self, symbol: &Symbol, on: NaiveDate) -> Symbol {
        let (mut symbol, mut on) = (symbol.clone(), on);
        let mut seen = BTreeSet::new();
        while seen.insert(symbol.clone()) {
            let Some((date, renamed)) = self
                .0
                .get(&symbol)
                .and_then(|changes| changes.iter().find(|(date, _)| *date >= on))
            else {
                break;
            };
            (symbol, on) = (renamed.clone(), *date);
        }
        symbol
    }

    fn former(&self, symbols: &[Symbol]) -> Vec<Symbol> {
        let mut wanted: BTreeSet<Symbol> = symbols.iter().cloned().collect();
        loop {
            let before = wanted.len();
            for (old, changes) in &self.0 {
                if changes.iter().any(|(_, new)| wanted.contains(new)) {
                    wanted.insert(old.clone());
                }
            }
            if wanted.len() == before {
                return wanted.into_iter().collect();
            }
        }
    }

    pub(crate) fn rekey(&self, mut action: CorporateAction) -> CorporateAction {
        let on = action.ex_date;
        action.symbol = self.current(&action.symbol, on);
        match &mut action.kind {
            CorporateActionKind::SpinOff { new_symbol, .. } => {
                *new_symbol = self.current(new_symbol, on);
            }
            CorporateActionKind::StockMerger { acquirer, .. } => {
                *acquirer = self.current(acquirer, on);
            }
            _ => {}
        }
        action
    }
}

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
            return Ok(Work::waiting("no instruments to price"));
        }
        let today = now.exchange_date();
        let through = today + ANNOUNCED_HORIZON;
        let history_start = self.config.history_start;
        let mut batch = Batch::default();
        if self.name_changes(now, alpaca, &mut batch, through).await? {
            return Ok(batch.finish("corporate actions"));
        }
        let renames = Renames::new(&self.market.actions_reaching_other_symbols().await?);
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
                alpaca
                    .corporate_actions(&renames.former(&scope), start, end)
                    .await
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
                    (from..=through).contains(&action.ex_date)
                        && !matches!(action.kind, CorporateActionKind::NameChange { .. })
                })
                .map(|action| renames.rekey(action))
                .filter(|action| members.contains(&action.symbol))
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

    async fn name_changes(
        &self,
        now: Timestamp,
        alpaca: &AlpacaClient,
        batch: &mut Batch,
        through: NaiveDate,
    ) -> Result<bool, IngestError> {
        let today = now.exchange_date();
        let history_start = self.config.history_start;
        let backfilled = self.market.coverage().await?.into_iter().any(|row| {
            row.dataset == Dataset::CorporateActions
                && row.source == DataSource::Alpaca
                && row.symbol.is_none()
                && row.first <= history_start
        });
        let from = match backfilled {
            true => (today - REFRESH_WINDOW).max(history_start),
            false => history_start,
        };
        let key = FetchKey::new(
            DataSource::Alpaca,
            Dataset::CorporateActions,
            None,
            Some((from, through)),
        );
        let fetched = alpaca
            .name_changes(from - PROCESS_DATE_SLACK, through + PROCESS_DATE_SLACK)
            .await;
        let fetched = match fetched {
            Ok(actions) => actions,
            Err(error) => {
                self.record_failure(now, &key, &error).await?;
                return Ok(batch.source_error("name changes", &error));
            }
        };
        let fetched: Vec<CorporateAction> = fetched
            .into_iter()
            .filter(|action| {
                (from..=through).contains(&action.ex_date)
                    && matches!(action.kind, CorporateActionKind::NameChange { .. })
            })
            .collect();
        let count = fetched.len();
        let summary = self
            .market
            .replace_name_changes(
                DataSource::Alpaca,
                fetched,
                ActionWindow {
                    symbols: Vec::new(),
                    from,
                    through,
                    covered_to: (!backfilled).then_some(today),
                },
            )
            .await?;
        self.record_fetched(now, &key, count).await?;
        batch.success(summary.rows);
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> NaiveDate {
        text.parse().unwrap()
    }

    fn renamed(old: &str, new: &str, on: &str) -> CorporateAction {
        CorporateAction {
            symbol: Symbol::new(old),
            ex_date: date(on),
            kind: CorporateActionKind::NameChange {
                new_symbol: Symbol::new(new),
            },
        }
    }

    #[test]
    fn an_action_is_keyed_by_the_ticker_its_company_trades_under_today() {
        let renames = Renames::new(&[
            renamed("SYMC", "NLOK", "2019-11-04"),
            renamed("NLOK", "GEN", "2022-11-08"),
            renamed("FB", "META", "2022-06-09"),
            renamed("UTX", "RTX", "2020-04-03"),
        ]);
        let spin_off = renames.rekey(CorporateAction {
            symbol: Symbol::new("UTX"),
            ex_date: date("2020-04-03"),
            kind: CorporateActionKind::SpinOff {
                new_symbol: Symbol::new("OTIS"),
                old_rate: 2.into(),
                new_rate: 1.into(),
            },
        });
        assert_eq!(spin_off.symbol, Symbol::new("RTX"));
        assert_eq!(
            renames.current(&Symbol::new("NLOK"), date("2020-02-03")),
            Symbol::new("GEN")
        );
        assert_eq!(
            renames.current(&Symbol::new("FB"), date("2021-12-14")),
            Symbol::new("META")
        );
        assert_eq!(
            renames.current(&Symbol::new("FB"), date("2025-09-25")),
            Symbol::new("FB"),
            "a dividend of the fund that took the ticker later stays with it"
        );
        assert_eq!(
            renames.former(&[Symbol::new("GEN")]),
            vec![Symbol::new("GEN"), Symbol::new("NLOK"), Symbol::new("SYMC")]
        );
    }
}
