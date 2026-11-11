use buizz_core::market::{Bar, DataSource, Dataset, Symbol, Timeframe};
use buizz_core::ops::FetchStatus;
use buizz_core::time::Timestamp;
use buizz_market::ActionWindow;
use buizz_sources::tiingo::{
    OverlapVerdict, REQUESTS_PER_DAY, SYMBOLS_PER_MONTH, normalize, validate_overlap,
};
use chrono::TimeDelta;

use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError, day_start};

const OVERLAP_WINDOW_DAYS: i64 = 45;
const SYMBOLS_PER_RUN: u64 = 3;
const MONTH: TimeDelta = TimeDelta::days(30);
const DAY: TimeDelta = TimeDelta::days(1);

impl Ingest {
    pub(crate) async fn tiingo_history(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(tiingo) = &self.sources.tiingo else {
            return Ok(Work::unconfigured("Tiingo"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Tiingo).await? {
            return Ok(work);
        }
        let history_start = self.config.history_start;
        let calendar = self
            .market
            .calendar(history_start, now.exchange_date())
            .await?;
        let alpaca = self
            .symbol_coverage(Dataset::Bars1d, DataSource::Alpaca)
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
        let mut batch = Batch::default();
        let mut remaining = room;
        for symbol in self.priced_symbols().await? {
            if remaining == 0 {
                break;
            }
            let Some(first) = alpaca.get(&symbol).map(|coverage| coverage.first) else {
                continue;
            };
            let gap_end = first - DAY;
            if calendar.between(history_start, gap_end).is_empty() {
                continue;
            }
            if self.tiingo_settled(now, &symbol).await? {
                continue;
            }
            remaining -= 1;
            let key = FetchKey::new(
                DataSource::Tiingo,
                Dataset::Bars1d,
                Some(&symbol),
                Some((history_start, gap_end)),
            );
            let window_end = first + TimeDelta::days(OVERLAP_WINDOW_DAYS);
            let rows = match tiingo.daily(&symbol, history_start, window_end).await {
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
                .filter(|bar| bar.start.exchange_date() >= first)
                .cloned()
                .collect();
            let rejection = match validate_overlap(&alpaca_bars, &overlap) {
                OverlapVerdict::Trusted { .. } => None,
                OverlapVerdict::Insufficient { overlap } => Some(format!(
                    "only {overlap} sessions overlap Alpaca, too few to trust Tiingo"
                )),
                OverlapVerdict::Rejected { reason } => Some(reason),
            };
            if let Some(reason) = rejection {
                self.record_rejection(now, &key, &reason).await?;
                batch.failure(symbol.as_str(), &format!("Tiingo rejected, {reason}"));
                continue;
            }
            let bars: Vec<Bar> = bars
                .into_iter()
                .filter(|bar| bar.start.exchange_date() < first)
                .collect();
            let actions = actions
                .into_iter()
                .filter(|action| action.ex_date < first)
                .collect::<Vec<_>>();
            let fetched = bars.len();
            let written = self
                .market
                .upsert_bars(DataSource::Tiingo, Timeframe::Day, bars)
                .await?;
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
            self.record_fetched(now, &key, fetched).await?;
            batch.success(written.rows);
        }
        Ok(batch.finish("Tiingo history"))
    }

    async fn tiingo_settled(&self, now: Timestamp, symbol: &Symbol) -> Result<bool, IngestError> {
        let latest = self
            .state
            .recent_fetches(DataSource::Tiingo, Dataset::Bars1d, Some(symbol.clone()), 1)
            .await?;
        Ok(latest.first().is_some_and(|fetch| {
            fetch.retry_after.is_some_and(|until| until > now)
                || matches!(fetch.status, FetchStatus::Ok | FetchStatus::Empty)
        }))
    }
}
