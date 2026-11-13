use std::collections::{BTreeMap, BTreeSet, HashSet};

use buizz_core::listing::Listings;
use buizz_core::market::{
    CorporateAction, CorporateActionKind, DataSource, Dataset, Symbol, Timeframe,
};
use buizz_core::ops::{ActionDisagreement, ActionVerdict, CheckedAction, DataWarning};
use buizz_core::time::Timestamp;
use buizz_market::ActionWindow;
use buizz_sources::alpaca::{ACTIONS_SYMBOLS_PER_REQUEST, AlpacaClient};
use chrono::{NaiveDate, TimeDelta};
use rust_decimal::Decimal;

use crate::crosscheck::{
    SplitEvidence, checked_as, repeated_dividends, split_evidence, unpriced_dividend,
};
use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError, day_start};

const PROCESS_DATE_SLACK: TimeDelta = TimeDelta::days(30);
const REFRESH_WINDOW: TimeDelta = TimeDelta::days(90);
const ANNOUNCED_HORIZON: TimeDelta = TimeDelta::days(90);
const BACKFILL_REQUESTS_PER_RUN: usize = 40;
const SPLIT_WINDOW: TimeDelta = TimeDelta::days(14);

struct TiingoChecks {
    checks: BTreeMap<(Symbol, NaiveDate, CheckedAction), ActionDisagreement>,
    checked_through: BTreeMap<Symbol, NaiveDate>,
}

impl Ingest {
    pub(crate) async fn corporate_actions(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(alpaca) = &self.sources.alpaca else {
            return Ok(Work::unconfigured("Alpaca"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Alpaca).await? {
            return Ok(work);
        }
        let mut symbols: Vec<Symbol> = self
            .priced_symbols()
            .await?
            .into_iter()
            .map(|priced| priced.symbol)
            .collect();
        let priced: BTreeSet<Symbol> = symbols.iter().cloned().collect();
        let ended: BTreeSet<Symbol> = self
            .market
            .instruments()
            .await?
            .into_iter()
            .filter_map(|instrument| Some(instrument.symbol.ended_listing()?.0))
            .filter(|ticker| !priced.contains(ticker))
            .collect();
        symbols.extend(ended);
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
        let renames = Listings::new(&self.market.actions_reaching_other_symbols().await?, [], []);
        let tiingo = self.tiingo_checks(history_start, through).await?;
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
                    (history_start..=through).contains(&action.ex_date)
                        && !matches!(action.kind, CorporateActionKind::NameChange { .. })
                })
                .map(|action| renames.rekey(action))
                .filter(|action| members.contains(&action.symbol))
                .collect();
            let count = fetched.len();
            let (fetched, rejected) = self.vetted(fetched, today, &tiingo).await?;
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
            if !rejected.is_empty() {
                self.market
                    .record_action_disagreements(DataSource::Alpaca, rejected)
                    .await?;
            }
            self.record_fetched(now, &key, count).await?;
            batch.success(summary.rows);
        }
        Ok(batch.finish("corporate actions"))
    }

    async fn distributed(
        &self,
        actions: &[CorporateAction],
        split: &CorporateAction,
    ) -> Result<Decimal, IngestError> {
        let mut distributed = Decimal::ZERO;
        let same_day = actions
            .iter()
            .filter(|action| action.symbol == split.symbol && action.ex_date == split.ex_date);
        for action in same_day {
            match &action.kind {
                CorporateActionKind::CashDividend { rate } => distributed += rate,
                CorporateActionKind::SpinOff {
                    new_symbol,
                    old_rate,
                    new_rate,
                } => {
                    let opened = self
                        .market
                        .bars_between(
                            Timeframe::Day,
                            vec![new_symbol.clone()],
                            day_start(split.ex_date),
                            day_start(split.ex_date + TimeDelta::days(1)),
                        )
                        .await?;
                    if let Some(bar) = opened.first() {
                        distributed += bar.open * new_rate / old_rate;
                    }
                }
                _ => {}
            }
        }
        Ok(distributed)
    }

    async fn tiingo_checks(
        &self,
        from: NaiveDate,
        through: NaiveDate,
    ) -> Result<TiingoChecks, IngestError> {
        let checks = self
            .market
            .action_disagreements(None, from, through)
            .await?
            .into_iter()
            .filter_map(|warning| match warning {
                DataWarning::ActionDisagreement(check) => {
                    Some(((check.symbol.clone(), check.ex_date, check.action), check))
                }
                _ => None,
            })
            .collect();
        let checked_through = self
            .symbol_coverage(Dataset::CorporateActions, DataSource::Tiingo)
            .await?
            .into_iter()
            .map(|(symbol, coverage)| (symbol, coverage.updated_at.exchange_date()))
            .collect();
        Ok(TiingoChecks {
            checks,
            checked_through,
        })
    }

    async fn vetted(
        &self,
        actions: Vec<CorporateAction>,
        today: NaiveDate,
        tiingo: &TiingoChecks,
    ) -> Result<(Vec<CorporateAction>, Vec<ActionDisagreement>), IngestError> {
        let mut rejected = Vec::new();
        let mut dropped = repeated_dividends(&actions);
        let mut moved: BTreeMap<usize, NaiveDate> = BTreeMap::new();
        let taken_by_tiingo = |action: &CorporateAction, checked: CheckedAction| {
            tiingo
                .checks
                .get(&(action.symbol.clone(), action.ex_date, checked))
                .is_some_and(|check| check.kept == ActionVerdict::Tiingo)
        };
        let wanted: Vec<(Symbol, NaiveDate)> = actions
            .iter()
            .filter(|action| {
                action.ex_date <= today
                    && matches!(action.kind, CorporateActionKind::CashDividend { .. })
            })
            .map(|action| (action.symbol.clone(), action.ex_date))
            .collect();
        let prices = self.market.ex_date_prices(wanted).await?;
        for (index, action) in actions.iter().enumerate() {
            if dropped.contains(&index) {
                continue;
            }
            if checked_as(action, CheckedAction::Split)
                && taken_by_tiingo(action, CheckedAction::Split)
            {
                dropped.insert(index);
                continue;
            }
            match action.kind {
                CorporateActionKind::CashDividend { rate } => {
                    if taken_by_tiingo(action, CheckedAction::CashDividend) {
                        dropped.insert(index);
                    } else if let Some((close, open)) =
                        prices.get(&(action.symbol.clone(), action.ex_date))
                        && unpriced_dividend(rate, *close, *open)
                    {
                        dropped.insert(index);
                        rejected.push(ActionDisagreement {
                            symbol: action.symbol.clone(),
                            ex_date: action.ex_date,
                            action: CheckedAction::CashDividend,
                            alpaca: Some(rate),
                            tiingo: None,
                            kept: ActionVerdict::Prices,
                        });
                    }
                }
                CorporateActionKind::Split { old_rate, new_rate } if action.ex_date <= today => {
                    let tiingo_split = match tiingo.checks.get(&(
                        action.symbol.clone(),
                        action.ex_date,
                        CheckedAction::Split,
                    )) {
                        Some(check) => check.tiingo.is_some(),
                        None => tiingo
                            .checked_through
                            .get(&action.symbol)
                            .is_some_and(|through| action.ex_date <= *through),
                    };
                    if tiingo_split {
                        continue;
                    }
                    let bars = self
                        .market
                        .bars_between(
                            Timeframe::Day,
                            vec![action.symbol.clone()],
                            day_start(action.ex_date - SPLIT_WINDOW),
                            day_start(action.ex_date + SPLIT_WINDOW),
                        )
                        .await?;
                    let distributed = self.distributed(&actions, action).await?;
                    let ratio = new_rate / old_rate;
                    let other_splits: Vec<NaiveDate> = actions
                        .iter()
                        .filter(|other| {
                            other.symbol == action.symbol
                                && other.ex_date != action.ex_date
                                && matches!(other.kind, CorporateActionKind::Split { .. })
                        })
                        .map(|other| other.ex_date)
                        .collect();
                    let evidence =
                        split_evidence(&bars, action.ex_date, ratio, distributed, &other_splits);
                    match evidence {
                        SplitEvidence::Shown | SplitEvidence::Unclear => continue,
                        SplitEvidence::Moved(date) => {
                            moved.insert(index, date);
                        }
                        SplitEvidence::Refuted => {
                            dropped.insert(index);
                        }
                    }
                    rejected.push(ActionDisagreement {
                        symbol: action.symbol.clone(),
                        ex_date: action.ex_date,
                        action: CheckedAction::Split,
                        alpaca: Some(ratio),
                        tiingo: None,
                        kept: ActionVerdict::Prices,
                    });
                }
                _ => {}
            }
        }
        let kept = actions
            .into_iter()
            .enumerate()
            .filter(|(index, _)| !dropped.contains(index))
            .map(|(index, action)| CorporateAction {
                ex_date: moved.get(&index).copied().unwrap_or(action.ex_date),
                ..action
            })
            .collect();
        Ok((kept, rejected))
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
        let key = FetchKey::new(
            DataSource::Alpaca,
            Dataset::CorporateActions,
            None,
            Some((history_start, through)),
        );
        let mergers = alpaca
            .stock_mergers(
                history_start - PROCESS_DATE_SLACK,
                through + PROCESS_DATE_SLACK,
            )
            .await;
        let mergers = match mergers {
            Ok(actions) => actions,
            Err(error) => {
                self.record_failure(now, &key, &error).await?;
                return Ok(batch.source_error("stock mergers", &error));
            }
        };
        let renames = Listings::new(&self.market.actions_reaching_other_symbols().await?, [], []);
        let mergers: Vec<CorporateAction> = mergers
            .into_iter()
            .filter(|action| {
                (history_start..=through).contains(&action.ex_date)
                    && matches!(&action.kind, CorporateActionKind::StockMerger {
                        old_rate,
                        new_rate,
                        cash_rate,
                        ..
                    } if old_rate == new_rate && cash_rate.is_zero())
            })
            .map(|action| renames.rekey(action))
            .collect();
        let count = mergers.len();
        let summary = self
            .market
            .replace_stock_mergers(
                DataSource::Alpaca,
                mergers,
                ActionWindow {
                    symbols: Vec::new(),
                    from: history_start,
                    through,
                    covered_to: None,
                },
            )
            .await?;
        self.record_fetched(now, &key, count).await?;
        batch.success(summary.rows);
        batch.success(self.curate().await?);
        let withdrawn = self
            .market
            .withdraw_shadowed_bars(DataSource::Alpaca, history_start, today)
            .await?;
        batch.success(withdrawn.rows);
        Ok(false)
    }
}
