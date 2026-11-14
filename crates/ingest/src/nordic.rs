use std::collections::BTreeMap;

use buizz_core::market::{
    DataSource, Dataset, NordicBar, NordicBars, NordicListing, NordicSplit, StoredNordicBar, Symbol,
};
use buizz_core::time::Timestamp;
use buizz_market::NordicWrite;
use buizz_sources::ecb::{CURRENCIES, FIRST_RATE};
use buizz_sources::euronext::{self, EuronextClient};
use buizz_sources::http::SourceError;
use buizz_sources::nasdaq_nordic::{self, NasdaqNordicClient};
use chrono::{Datelike, Months, NaiveDate, NaiveTime, TimeDelta, Weekday};
use chrono_tz::Europe::Stockholm;
use rust_decimal::Decimal;

use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError};

const READ_FROM: NaiveTime = NaiveTime::from_hms_opt(18, 0, 0).expect("a time");
const OVERLAP: TimeDelta = TimeDelta::days(14);
const SAME_CLOSE: Decimal = Decimal::from_parts(5, 0, 0, false, 3);
const SAME_RATIO: Decimal = Decimal::from_parts(1, 0, 0, false, 2);
const RATIO_PLACES: u32 = 6;

#[derive(Clone, Copy)]
pub(crate) enum Exchange<'a> {
    Nasdaq(&'a NasdaqNordicClient),
    Euronext(&'a EuronextClient),
}

impl Exchange<'_> {
    pub(crate) fn source(self) -> DataSource {
        match self {
            Self::Nasdaq(_) => DataSource::NasdaqNordic,
            Self::Euronext(_) => DataSource::Euronext,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Nasdaq(_) => "Nasdaq Nordic",
            Self::Euronext(_) => "Euronext Oslo",
        }
    }

    async fn listings(self) -> Result<Vec<NordicListing>, SourceError> {
        match self {
            Self::Nasdaq(client) => client.listings().await,
            Self::Euronext(client) => client.oslo_listings().await,
        }
    }

    async fn daily_bars(
        self,
        listing: &Symbol,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<NordicBars, SourceError> {
        match self {
            Self::Nasdaq(client) => client.daily_bars(listing, from, to).await,
            Self::Euronext(client) => client.daily_bars(listing, from, to).await,
        }
    }
}

pub(crate) fn history_start(source: DataSource, today: NaiveDate) -> NaiveDate {
    let months = match source {
        DataSource::Euronext => euronext::HISTORY_MONTHS,
        _ => nasdaq_nordic::HISTORY_MONTHS,
    };
    today
        .checked_sub_months(Months::new(months))
        .unwrap_or(today)
}

fn last_closed_day(now: Timestamp) -> NaiveDate {
    let local = now.to_datetime().with_timezone(&Stockholm);
    let mut day = local.date_naive();
    if local.time() < READ_FROM {
        day = day.pred_opt().unwrap_or(day);
    }
    while matches!(day.weekday(), Weekday::Sat | Weekday::Sun) {
        day = day.pred_opt().unwrap_or(day);
    }
    day
}

struct Reconciled {
    bars: Vec<NordicBar>,
    split: Option<(NaiveDate, Decimal)>,
    revised: usize,
}

fn reconcile(
    stored: &[StoredNordicBar],
    splits: &[NordicSplit],
    fetched: &[NordicBar],
) -> Reconciled {
    let expected: BTreeMap<NaiveDate, (NordicBar, Decimal)> = stored
        .iter()
        .map(|kept| {
            let factor = splits
                .iter()
                .filter(|split| split.date > kept.bar.date && split.detected_at > kept.recorded_at)
                .map(|split| split.ratio)
                .product();
            (kept.bar.date, (kept.bar, factor))
        })
        .collect();
    let mut fresh = Vec::new();
    let mut matched = Vec::new();
    let mut changed: Vec<(NordicBar, Option<Decimal>)> = Vec::new();
    for bar in fetched {
        let Some((kept, factor)) = expected.get(&bar.date) else {
            fresh.push(*bar);
            continue;
        };
        let ratio = kept
            .close
            .checked_div(*factor)
            .and_then(|close| close.checked_div(bar.close));
        let same = if *factor == Decimal::ONE {
            kept == bar
        } else {
            ratio.is_some_and(|ratio| (ratio - Decimal::ONE).abs() <= SAME_CLOSE)
        };
        if same {
            matched.push(bar.date);
        } else {
            changed.push((*bar, ratio));
        }
    }
    let shifted = |ratio: &Option<Decimal>| {
        ratio.is_some_and(|ratio| (ratio - Decimal::ONE).abs() > SAME_RATIO)
    };
    let split = split_in(&changed, &matched, fetched, shifted);
    let revisions: Vec<NordicBar> = changed
        .iter()
        .filter(|(_, ratio)| split.is_none() || !shifted(ratio))
        .map(|(bar, _)| *bar)
        .collect();
    let revised = revisions.len();
    let mut bars: Vec<NordicBar> = fresh.into_iter().chain(revisions).collect();
    bars.sort_by_key(|bar| bar.date);
    Reconciled {
        bars,
        split,
        revised,
    }
}

fn split_in(
    changed: &[(NordicBar, Option<Decimal>)],
    matched: &[NaiveDate],
    fetched: &[NordicBar],
    shifted: impl Fn(&Option<Decimal>) -> bool,
) -> Option<(NaiveDate, Decimal)> {
    let mut ratios: Vec<Decimal> = changed
        .iter()
        .filter(|(_, ratio)| shifted(ratio))
        .filter_map(|(_, ratio)| *ratio)
        .collect();
    ratios.sort();
    let ratio = *ratios.get(ratios.len() / 2)?;
    let last = changed
        .iter()
        .filter(|(_, ratio)| shifted(ratio))
        .map(|(bar, _)| bar.date)
        .max()?;
    let consistent = ratios
        .iter()
        .all(|each| ((*each - ratio) / ratio).abs() <= SAME_RATIO);
    let contiguous = matched.iter().all(|date| *date > last)
        && changed
            .iter()
            .all(|(bar, ratio)| shifted(ratio) || bar.date > last);
    let date = fetched
        .iter()
        .map(|bar| bar.date)
        .find(|date| *date > last)?;
    (consistent && contiguous).then(|| (date, ratio.round_dp(RATIO_PLACES)))
}

#[derive(Default)]
struct Tally {
    skipped: usize,
    revised: usize,
    splits: usize,
}

impl Tally {
    fn describe(&self, listed: usize, changed: usize) -> String {
        let mut parts = vec![format!("{listed} listed, {changed} new or changed")];
        for (count, what) in [
            (self.skipped, "skipped without a full price"),
            (self.revised, "revised"),
            (self.splits, "split"),
        ] {
            if count > 0 {
                parts.push(format!("{count} {what}"));
            }
        }
        parts.join(", ")
    }
}

impl Ingest {
    pub(crate) async fn nasdaq_nordic(&self, now: Timestamp) -> Result<Work, IngestError> {
        match &self.sources.nasdaq_nordic {
            Some(client) => self.nordic_prices(now, Exchange::Nasdaq(client)).await,
            None => Ok(Work::unconfigured("Nasdaq Nordic")),
        }
    }

    pub(crate) async fn euronext_oslo(&self, now: Timestamp) -> Result<Work, IngestError> {
        match &self.sources.euronext {
            Some(client) => self.nordic_prices(now, Exchange::Euronext(client)).await,
            None => Ok(Work::unconfigured("Euronext")),
        }
    }

    pub(crate) fn nordic_wanted(&self, listing: &NordicListing) -> bool {
        self.config.only.as_ref().is_none_or(|only| {
            only.contains(&listing.listing) || only.contains(&Symbol::new(&listing.symbol))
        })
    }

    async fn nordic_prices(
        &self,
        now: Timestamp,
        exchange: Exchange<'_>,
    ) -> Result<Work, IngestError> {
        let (source, name) = (exchange.source(), exchange.name());
        let through = last_closed_day(now);
        let read_through = self
            .market
            .coverage()
            .await?
            .into_iter()
            .find(|row| {
                row.dataset == Dataset::NordicBars1d && row.source == source && row.symbol.is_none()
            })
            .map(|row| row.last);
        if read_through >= Some(through) {
            return Ok(Work::waiting(format!(
                "{name}: closes through {through} read; the next are read after 18:00 Stockholm time"
            )));
        }
        if let Some(work) = self.backoff_work(now, source).await? {
            return Ok(work);
        }
        let key = FetchKey::new(
            source,
            Dataset::NordicListings,
            None,
            Some((through, through)),
        );
        let listings = match exchange.listings().await {
            Ok(listings) => listings,
            Err(error) => return self.source_failure(now, &key, &error).await,
        };
        self.record_fetched(now, &key, listings.len()).await?;
        let previous = self.market.nordic_listed(source).await?;
        let changed: Vec<NordicListing> = listings
            .iter()
            .filter(|listing| !previous.contains(listing))
            .cloned()
            .collect();
        let changes = changed.len();
        self.market
            .append_nordic_listings(
                source,
                through,
                now,
                changed,
                listings
                    .iter()
                    .map(|listing| listing.listing.clone())
                    .collect(),
            )
            .await?;
        let last_dates = self.market.nordic_bar_dates(source).await?;
        let mut wanted: Vec<&NordicListing> = listings
            .iter()
            .filter(|listing| self.nordic_wanted(listing))
            .filter(|listing| {
                last_dates
                    .get(&listing.listing)
                    .is_none_or(|last| *last < through)
            })
            .collect();
        wanted.sort_by_key(|listing| last_dates.get(&listing.listing));
        let backfill_from = history_start(source, through);
        let mut batch = Batch::default();
        let mut tally = Tally::default();
        let mut complete = true;
        for listing in wanted {
            if self.stopping() {
                batch.more_remains(format!("{name} from {}", listing.listing));
                complete = false;
                break;
            }
            let from = last_dates
                .get(&listing.listing)
                .map_or(backfill_from, |last| *last - OVERLAP);
            let key = FetchKey::new(
                source,
                Dataset::NordicBars1d,
                Some(&listing.listing),
                Some((from, through)),
            );
            let fetched = match exchange.daily_bars(&listing.listing, from, through).await {
                Ok(fetched) => fetched,
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    if batch.source_error(listing.listing.as_str(), &error) {
                        complete = false;
                        break;
                    }
                    continue;
                }
            };
            self.record_fetched(now, &key, fetched.bars.len()).await?;
            let (stored, splits) = self
                .market
                .nordic_history(source, listing.listing.clone(), from)
                .await?;
            let reconciled = reconcile(&stored, &splits, &fetched.bars);
            tally.skipped += fetched.skipped;
            tally.revised += reconciled.revised;
            tally.splits += usize::from(reconciled.split.is_some());
            if reconciled.bars.is_empty() && reconciled.split.is_none() {
                batch.success(0);
                continue;
            }
            let summary = self
                .market
                .append_nordic_bars(NordicWrite {
                    source,
                    listing: listing.listing.clone(),
                    currency: listing.currency.clone(),
                    at: now,
                    bars: reconciled.bars,
                    split: reconciled.split.map(|(date, ratio)| NordicSplit {
                        date,
                        ratio,
                        detected_at: now,
                    }),
                })
                .await?;
            batch.success(summary.rows);
        }
        if complete {
            self.market
                .record_seen(source, Dataset::NordicBars1d, through)
                .await?;
        }
        let mut work = batch.finish(name);
        work.message = format!(
            "{}; {}",
            work.message,
            tally.describe(listings.len(), changes)
        );
        Ok(work)
    }

    pub(crate) async fn fx_rates(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(ecb) = &self.sources.ecb else {
            return Ok(Work::unconfigured("ECB"));
        };
        let stored = self.market.fx_rate_dates().await?;
        let since = CURRENCIES
            .iter()
            .map(|currency| {
                stored
                    .get(*currency)
                    .and_then(|last| last.succ_opt())
                    .unwrap_or(FIRST_RATE)
            })
            .min()
            .unwrap_or(FIRST_RATE);
        let keys: Vec<(&str, FetchKey)> = CURRENCIES
            .iter()
            .map(|currency| {
                let series = Symbol::new(currency);
                (
                    *currency,
                    FetchKey::new(DataSource::Ecb, Dataset::FxRates, Some(&series), None),
                )
            })
            .collect();
        let rates = match ecb.euro_rates(since).await {
            Ok(rates) => rates,
            Err(error) => {
                for (_, key) in &keys {
                    self.record_failure(now, key, &error).await?;
                }
                return Ok(Work::from_failure(&error));
            }
        };
        let rates: Vec<_> = rates
            .into_iter()
            .filter(|rate| {
                stored
                    .get(&rate.currency)
                    .is_none_or(|last| rate.date > *last)
            })
            .collect();
        for (currency, key) in &keys {
            let rows = rates
                .iter()
                .filter(|rate| rate.currency == *currency)
                .count();
            self.record_fetched(now, key, rows).await?;
        }
        if rates.is_empty() {
            return Ok(Work::noop("fx rates: nothing new"));
        }
        let summary = self
            .market
            .append_fx_rates(DataSource::Ecb, now, rates)
            .await?;
        Ok(Work::ok(
            summary.rows,
            format!("fx rates: {} rows", summary.rows),
        ))
    }
}
