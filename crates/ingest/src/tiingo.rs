use std::collections::{BTreeMap, BTreeSet};

use buizz_core::market::{
    Bar, Calendar, CorporateAction, CorporateActionKind, DataSource, Dataset, StockIndex, Symbol,
    Timeframe,
};
use buizz_core::ops::{ActionDisagreement, ActionVerdict, CheckedAction, FetchStatus};
use buizz_core::run::PeriodPreset;
use buizz_core::time::Timestamp;
use buizz_market::ActionWindow;
use buizz_sources::tiingo::{
    OverlapVerdict, REQUESTS_PER_DAY, SYMBOLS_PER_MONTH, TiingoClient, normalize, validate_overlap,
};
use chrono::{NaiveDate, TimeDelta};
use rust_decimal::Decimal;

use crate::crosscheck::{Adoption, SplitEvidence, checked_as, cross_check, split_evidence};
use crate::fetch::{Batch, FetchKey, Work, read, waiting};
use crate::{Ingest, IngestError, Priced, day_start};

const OVERLAP_WINDOW_DAYS: i64 = 45;
const SYMBOLS_PER_RUN: u64 = 3;
const MONTH: TimeDelta = TimeDelta::days(30);
const DAY: TimeDelta = TimeDelta::days(1);
const LOOKBACK: TimeDelta = TimeDelta::days(365);
const REJECTION_RETRY: TimeDelta = TimeDelta::days(365);
const PROBE_REACH: TimeDelta = TimeDelta::days(14);
const PROBES_REMEMBERED: u32 = 50;
const LIQUID_DOLLAR_VOLUME: Decimal = Decimal::from_parts(1_000_000, 0, 0, false, 0);

enum Candidate {
    Untried,
    Rejected,
}

impl Ingest {
    pub(crate) async fn tiingo_history(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(tiingo) = &self.sources.tiingo else {
            return Ok(Work::unconfigured("Tiingo"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Tiingo).await? {
            return Ok(work);
        }
        self.market.normalize_splits(DataSource::Tiingo).await?;
        let history_start = self.config.history_start;
        let today = now.exchange_date();
        let calendar = self.market.calendar(history_start, today).await?;
        let alpaca = self
            .symbol_coverage(Dataset::Bars1d, DataSource::Alpaca)
            .await?;
        let alpaca_actions = self
            .symbol_coverage(Dataset::CorporateActions, DataSource::Alpaca)
            .await?;
        let daily = self
            .state
            .fetch_usage(DataSource::Tiingo, now - DAY)
            .await?;
        let monthly = self
            .state
            .fetch_usage(DataSource::Tiingo, now - MONTH)
            .await?;
        let room = u64::from(REQUESTS_PER_DAY)
            .saturating_sub(daily.requests)
            .min(u64::from(SYMBOLS_PER_MONTH).saturating_sub(monthly.symbols))
            .min(SYMBOLS_PER_RUN);
        let Some(alpaca_floor) = alpaca.values().map(|coverage| coverage.first).min() else {
            return Ok(Work::waiting(
                "Tiingo history: no Alpaca history to extend yet",
            ));
        };
        if room == 0 {
            return Ok(Work::waiting(
                "Tiingo history: the free plan's request budget is used; it resumes as the window rolls",
            ));
        }
        let room = usize::try_from(room).unwrap_or(usize::MAX);
        let screened_from = PeriodPreset::TenYears.resolve(today).from;
        let listings = self.market.listings();
        let lookback: Vec<Symbol> = self
            .market
            .unpriced_members(
                StockIndex::Sp500,
                screened_from - LOOKBACK,
                screened_from.min(alpaca_floor) - DAY,
            )
            .await?
            .into_iter()
            .map(|gap| {
                listings
                    .listing(&gap.symbol, gap.from)
                    .unwrap_or(gap.symbol)
            })
            .collect();
        let priced = self.priced_symbols().await?;
        let mut seen = BTreeSet::new();
        let mut untried = Vec::new();
        let mut rejected = Vec::new();
        for symbol in lookback
            .into_iter()
            .chain(priced.into_iter().map(|Priced { symbol, .. }| symbol))
        {
            if untried.len() == room {
                break;
            }
            if !seen.insert(symbol.clone()) {
                continue;
            }
            let Some(first) = alpaca.get(&symbol).map(|coverage| coverage.first) else {
                continue;
            };
            if first != alpaca_floor || calendar.between(history_start, first - DAY).is_empty() {
                continue;
            }
            match self.tiingo_candidate(now, &symbol).await? {
                Some(Candidate::Untried) => untried.push((symbol, first)),
                Some(Candidate::Rejected) => rejected.push((symbol, first)),
                None => {}
            }
        }
        let mut batch = Batch::default();
        let extending: Vec<(Symbol, NaiveDate)> =
            untried.into_iter().chain(rejected).take(room).collect();
        let probes = room - extending.len();
        for (symbol, first) in extending {
            if self.stopping() {
                break;
            }
            let ticker = symbol
                .ended_listing()
                .map_or_else(|| symbol.clone(), |(ticker, _)| ticker);
            let gap_end = first - DAY;
            let key = FetchKey::new(
                DataSource::Tiingo,
                Dataset::Bars1d,
                Some(&symbol),
                Some((history_start, gap_end)),
            );
            let window_end = first + TimeDelta::days(OVERLAP_WINDOW_DAYS);
            let rows = match tiingo.daily(&ticker, history_start, today).await {
                Ok(rows) => rows,
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    if batch.source_error(symbol.as_str(), &error) {
                        break;
                    }
                    continue;
                }
            };
            let (bars, actions) = normalize(&symbol, &rows, &calendar);
            let alpaca_bars = self
                .market
                .bars_between(
                    Timeframe::Day,
                    vec![symbol.clone()],
                    day_start(first),
                    day_start(window_end + DAY),
                )
                .await?;
            let overlap: Vec<Bar> = bars
                .iter()
                .filter(|bar| (first..=window_end).contains(&bar.start.exchange_date()))
                .cloned()
                .collect();
            let agreement = match validate_overlap(&alpaca_bars, &overlap) {
                OverlapVerdict::Trusted(agreement) => agreement,
                OverlapVerdict::Insufficient { overlap } => {
                    let reason =
                        format!("only {overlap} sessions overlap Alpaca, too few to trust Tiingo");
                    self.record_rejection(now, &key, &reason).await?;
                    batch.failure(symbol.as_str(), &format!("Tiingo rejected, {reason}"));
                    continue;
                }
                OverlapVerdict::Rejected { reason } => {
                    self.record_rejection(now, &key, &reason).await?;
                    batch.failure(symbol.as_str(), &format!("Tiingo rejected, {reason}"));
                    continue;
                }
            };
            let (bars, quiet): (Vec<Bar>, Vec<Bar>) = bars
                .into_iter()
                .filter(|bar| bar.start.exchange_date() < first)
                .partition(|bar| bar.volume > 0);
            let (actions, alpaca_era): (Vec<CorporateAction>, Vec<CorporateAction>) = actions
                .into_iter()
                .partition(|action| action.ex_date < first);
            if alpaca_actions
                .get(&symbol)
                .is_some_and(|coverage| coverage.first <= history_start)
            {
                self.cross_check(&symbol, first, today, &alpaca_era).await?;
            }
            let fetched = bars.len();
            let written = self
                .market
                .upsert_bars(DataSource::Tiingo, Timeframe::Day, bars)
                .await?;
            if !quiet.is_empty() {
                self.market
                    .record_quiet_sessions(
                        DataSource::Tiingo,
                        quiet
                            .iter()
                            .map(|bar| (bar.symbol.clone(), bar.start.exchange_date()))
                            .collect(),
                    )
                    .await?;
            }
            self.market
                .replace_corporate_actions(
                    DataSource::Tiingo,
                    actions,
                    ActionWindow {
                        symbols: vec![symbol.clone()],
                        from: history_start,
                        through: gap_end,
                        covered_to: Some(gap_end),
                    },
                )
                .await?;
            self.record_agreed(now, &key, fetched, agreement.to_string())
                .await?;
            batch.success(written.rows);
        }
        if probes > 0 && !self.stopping() {
            self.probe_split_jumps(now, tiingo, &calendar, probes, &mut batch)
                .await?;
        }
        Ok(batch.finish("Tiingo history"))
    }

    async fn cross_check(
        &self,
        symbol: &Symbol,
        first: NaiveDate,
        today: NaiveDate,
        tiingo: &[CorporateAction],
    ) -> Result<(), IngestError> {
        let stored = self
            .market
            .corporate_actions(vec![symbol.clone()], first, today)
            .await?;
        let bars = self
            .market
            .bars_between(
                Timeframe::Day,
                vec![symbol.clone()],
                day_start(first),
                day_start(today + DAY),
            )
            .await?;
        let check = cross_check(&stored, tiingo, &bars);
        let mut adopted: BTreeMap<NaiveDate, Vec<Adoption>> = BTreeMap::new();
        for adoption in check.adopted {
            adopted.entry(adoption.ex_date).or_default().push(adoption);
        }
        for (date, adoptions) in adopted {
            let window = || ActionWindow {
                symbols: vec![symbol.clone()],
                from: date,
                through: date,
                covered_to: None,
            };
            let taken: Vec<CorporateAction> = adoptions
                .iter()
                .flat_map(|adoption| adoption.tiingo.iter().cloned())
                .collect();
            if !taken.is_empty() {
                self.market
                    .replace_corporate_actions(DataSource::Tiingo, taken, window())
                    .await?;
            }
            let kept = stored
                .iter()
                .filter(|action| {
                    action.ex_date == date
                        && !adoptions
                            .iter()
                            .any(|adoption| checked_as(action, adoption.action))
                })
                .cloned()
                .collect();
            self.market
                .replace_corporate_actions(DataSource::Alpaca, kept, window())
                .await?;
        }
        if !check.disagreements.is_empty() {
            self.market
                .record_action_disagreements(DataSource::Tiingo, check.disagreements)
                .await?;
        }
        Ok(())
    }

    async fn probe_split_jumps(
        &self,
        now: Timestamp,
        tiingo: &TiingoClient,
        calendar: &Calendar,
        budget: usize,
        batch: &mut Batch,
    ) -> Result<(), IngestError> {
        let jumps = self.market.split_like_jumps(LIQUID_DOLLAR_VOLUME).await?;
        let mut probed = 0;
        for (symbol, date) in jumps {
            if probed == budget || self.stopping() {
                break;
            }
            let tried = self
                .state
                .recent_fetches(
                    DataSource::Tiingo,
                    Dataset::CorporateActions,
                    Some(symbol.clone()),
                    PROBES_REMEMBERED,
                )
                .await?
                .iter()
                .any(|fetch| {
                    fetch.requested_from == Some(date) && (read(fetch) || fetch.at + MONTH > now)
                });
            if tried {
                continue;
            }
            probed += 1;
            let key = FetchKey::new(
                DataSource::Tiingo,
                Dataset::CorporateActions,
                Some(&symbol),
                Some((date, date)),
            );
            let rows = match tiingo
                .daily(&symbol, date - PROBE_REACH, date + PROBE_REACH)
                .await
            {
                Ok(rows) => rows,
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    if batch.source_error(symbol.as_str(), &error) {
                        break;
                    }
                    continue;
                }
            };
            let (_, actions) = normalize(&symbol, &rows, calendar);
            let bars = self
                .market
                .bars_between(
                    Timeframe::Day,
                    vec![symbol.clone()],
                    day_start(date - PROBE_REACH),
                    day_start(date + PROBE_REACH),
                )
                .await?;
            let split = actions.into_iter().find(|action| {
                action.ex_date == date
                    && matches!(action.kind, CorporateActionKind::Split { .. })
                    && action
                        .kind
                        .share_ratio()
                        .is_some_and(|(old_rate, new_rate)| {
                            split_evidence(&bars, date, new_rate / old_rate, Decimal::ZERO, &[])
                                == SplitEvidence::Shown
                        })
            });
            let found = usize::from(split.is_some());
            if let Some(split) = split {
                let ratio = split
                    .kind
                    .share_ratio()
                    .map(|(old_rate, new_rate)| new_rate / old_rate);
                let window = ActionWindow {
                    symbols: vec![symbol.clone()],
                    from: date,
                    through: date,
                    covered_to: None,
                };
                let written = self
                    .market
                    .replace_corporate_actions(DataSource::Tiingo, vec![split], window)
                    .await?;
                self.market
                    .record_action_disagreements(
                        DataSource::Tiingo,
                        vec![ActionDisagreement {
                            symbol: symbol.clone(),
                            ex_date: date,
                            action: CheckedAction::Split,
                            alpaca: None,
                            tiingo: ratio,
                            kept: ActionVerdict::Tiingo,
                        }],
                    )
                    .await?;
                batch.success(written.rows);
            }
            self.record_fetched(now, &key, found).await?;
        }
        Ok(())
    }

    async fn tiingo_candidate(
        &self,
        now: Timestamp,
        symbol: &Symbol,
    ) -> Result<Option<Candidate>, IngestError> {
        let Some(fetch) = self
            .latest_fetch(DataSource::Tiingo, Dataset::Bars1d, symbol)
            .await?
        else {
            return Ok(Some(Candidate::Untried));
        };
        let extended = read(&fetch)
            && fetch
                .requested_from
                .is_some_and(|from| from <= self.config.history_start);
        Ok(match fetch.status {
            FetchStatus::Rejected if fetch.at + REJECTION_RETRY <= now => Some(Candidate::Rejected),
            FetchStatus::Rejected => None,
            _ if extended || waiting(&fetch, now) => None,
            _ => Some(Candidate::Untried),
        })
    }
}
