use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use buizz_core::market::{Bar, Calendar, FundamentalMetric, FundamentalPoint, Symbol, Timeframe};
use buizz_core::time::Timestamp;
use chrono::NaiveDate;
use duckdb::Connection;
use rust_decimal::Decimal;

use crate::Result;
use crate::read::{
    BarQuery, MacroSource, Vintage, bars, daily_prices, fact_arrivals, fundamentals, is_visible,
    macro_vintages, sort_points,
};

const AHEAD_SESSIONS: usize = 63;

type Prices = BTreeMap<(Symbol, NaiveDate), (Decimal, Decimal)>;

#[derive(Default)]
pub(crate) struct Store {
    through: Option<Timestamp>,
    windows: BTreeMap<Symbol, Window>,
    prices: BTreeMap<(Symbol, NaiveDate), Option<(Decimal, Decimal)>>,
    vintages: BTreeMap<MacroSource, Arc<Vec<Vintage>>>,
    facts: BTreeMap<Symbol, Facts>,
}

#[derive(Default)]
struct Facts {
    arrivals: Vec<Timestamp>,
    metrics: BTreeMap<FundamentalMetric, (Timestamp, Vec<FundamentalPoint>)>,
}

impl Facts {
    fn fresh(&self, metric: FundamentalMetric, as_of: Timestamp) -> bool {
        self.metrics.get(&metric).is_some_and(|(read_at, _)| {
            let next = self
                .arrivals
                .get(self.arrivals.partition_point(|at| at <= read_at))
                .copied()
                .unwrap_or(Timestamp::MAX);
            *read_at <= as_of && as_of < next
        })
    }
}

struct Window {
    from: Timestamp,
    bars: Vec<Bar>,
    deepest: usize,
    asked: bool,
}

impl Window {
    fn latest(
        &self,
        calendar: &Calendar,
        since: Option<&Timestamp>,
        count: usize,
        as_of: Timestamp,
    ) -> Option<Vec<Bar>> {
        let until = self.bars.partition_point(|bar| bar.start <= as_of);
        let mut found: Vec<Bar> = self.bars[..until]
            .iter()
            .rev()
            .filter(|bar| since.is_none_or(|since| bar.start >= *since))
            .filter(|bar| is_visible(calendar, Timeframe::Day, bar, as_of))
            .take(count)
            .cloned()
            .collect();
        let first = calendar.sessions().first().map(|session| session.open);
        let exhausted = first.is_none_or(|first| self.from <= first)
            || since.is_some_and(|since| self.from <= *since);
        (found.len() >= count || exhausted).then(|| {
            found.reverse();
            found
        })
    }

    fn trim(&mut self, as_of: Timestamp) {
        let until = self.bars.partition_point(|bar| bar.start <= as_of);
        let keep = until.saturating_sub(self.deepest + 1);
        if keep > 0 {
            self.bars.drain(..keep);
            self.from = self.bars[0].start;
        }
    }
}

impl Store {
    pub(crate) fn visible(
        &mut self,
        connection: &Connection,
        calendar: &Calendar,
        listed_since: &BTreeMap<Symbol, Timestamp>,
        keys: Vec<Symbol>,
        count: usize,
        as_of: Timestamp,
    ) -> Result<BTreeMap<Symbol, Vec<Bar>>> {
        let keys: BTreeSet<Symbol> = keys.into_iter().collect();
        let sessions = calendar.sessions();
        let opened = sessions.partition_point(|session| session.open <= as_of);
        let open_at = |index: usize| sessions.get(index).map_or(Timestamp::MAX, |s| s.open);
        if self.through.is_none_or(|through| through < open_at(opened)) {
            self.windows
                .retain(|key, window| window.asked || keys.contains(key));
            let through = open_at(opened + AHEAD_SESSIONS);
            let held: Vec<Symbol> = self.windows.keys().cloned().collect();
            if let Some(from) = self.through {
                let mut ahead = stored(connection, &held, from, through)?;
                for (key, window) in &mut self.windows {
                    window.bars.extend(ahead.remove(key).unwrap_or_default());
                    window.asked = false;
                    window.trim(as_of);
                }
            }
            self.through = Some(through);
        }
        let through = self.through.unwrap_or(Timestamp::MAX);
        let fresh: Vec<Symbol> = keys
            .iter()
            .filter(|key| !self.windows.contains_key(*key))
            .cloned()
            .collect();
        let from = open_at(
            opened
                .saturating_sub(count + 1)
                .min(sessions.len().saturating_sub(1)),
        );
        let mut loaded = stored(connection, &fresh, from, through)?;
        for key in fresh {
            let bars = loaded.remove(&key).unwrap_or_default();
            self.windows.insert(
                key,
                Window {
                    from,
                    bars,
                    deepest: 0,
                    asked: false,
                },
            );
        }
        let mut visible = BTreeMap::new();
        let mut pending: Vec<Symbol> = keys.into_iter().collect();
        let mut lookback = count + 1;
        while !pending.is_empty() {
            let mut short = Vec::new();
            for key in pending {
                let Some(window) = self.windows.get_mut(&key) else {
                    continue;
                };
                window.asked = true;
                window.deepest = window.deepest.max(count);
                match window.latest(calendar, listed_since.get(&key), count, as_of) {
                    Some(bars) if bars.is_empty() => {}
                    Some(bars) => {
                        visible.insert(key, bars);
                    }
                    None => short.push(key),
                }
            }
            let Some(upto) = short
                .iter()
                .filter_map(|key| self.windows.get(key))
                .map(|window| window.from)
                .max()
            else {
                break;
            };
            let reached = short
                .iter()
                .filter_map(|key| self.windows.get(key))
                .map(|window| sessions.partition_point(|session| session.open < window.from))
                .min()
                .unwrap_or(0);
            lookback = (lookback * 2).max(opened.saturating_sub(reached) + 1);
            let from = open_at(opened.saturating_sub(lookback));
            let mut older = stored(connection, &short, from, upto)?;
            for key in &short {
                if let Some(window) = self.windows.get_mut(key) {
                    let mut bars: Vec<Bar> = older
                        .remove(key)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|bar| bar.start < window.from)
                        .collect();
                    bars.append(&mut window.bars);
                    window.bars = bars;
                    window.from = window.from.min(from);
                }
            }
            pending = short;
        }
        Ok(visible)
    }

    pub(crate) fn vintages(
        &mut self,
        connection: &Connection,
        source: MacroSource,
    ) -> Result<Arc<Vec<Vintage>>> {
        if let Some(vintages) = self.vintages.get(&source) {
            return Ok(Arc::clone(vintages));
        }
        let vintages = Arc::new(macro_vintages(connection, source, None)?);
        self.vintages.insert(source, Arc::clone(&vintages));
        Ok(vintages)
    }

    pub(crate) fn fundamentals(
        &mut self,
        connection: &Connection,
        symbols: &[Symbol],
        metrics: &[FundamentalMetric],
        as_of: Timestamp,
    ) -> Result<Vec<FundamentalPoint>> {
        let symbols: BTreeSet<Symbol> = symbols.iter().cloned().collect();
        let metrics: BTreeSet<FundamentalMetric> = metrics.iter().copied().collect();
        let unknown: Vec<Symbol> = symbols
            .iter()
            .filter(|symbol| !self.facts.contains_key(*symbol))
            .cloned()
            .collect();
        let mut arrivals = fact_arrivals(connection, &unknown)?;
        for symbol in unknown {
            let arrivals = arrivals.remove(&symbol).unwrap_or_default();
            self.facts.insert(
                symbol,
                Facts {
                    arrivals,
                    ..Facts::default()
                },
            );
        }
        let stale: Vec<Symbol> = symbols
            .iter()
            .filter(|symbol| {
                self.facts
                    .get(*symbol)
                    .is_none_or(|facts| !metrics.iter().all(|metric| facts.fresh(*metric, as_of)))
            })
            .cloned()
            .collect();
        let asked: Vec<FundamentalMetric> = metrics.iter().copied().collect();
        let mut read: BTreeMap<(Symbol, FundamentalMetric), Vec<FundamentalPoint>> =
            BTreeMap::new();
        for point in fundamentals(connection, &stale, &asked, as_of)? {
            read.entry((point.symbol.clone(), point.metric))
                .or_default()
                .push(point);
        }
        for symbol in stale {
            let facts = self.facts.entry(symbol.clone()).or_default();
            for metric in &metrics {
                let points = read.remove(&(symbol.clone(), *metric)).unwrap_or_default();
                facts.metrics.insert(*metric, (as_of, points));
            }
        }
        let mut points: Vec<FundamentalPoint> = symbols
            .iter()
            .filter_map(|symbol| self.facts.get(symbol))
            .flat_map(|facts| {
                metrics
                    .iter()
                    .filter_map(|metric| facts.metrics.get(metric))
                    .flat_map(|(_, points)| points.iter().cloned())
            })
            .collect();
        sort_points(&mut points);
        Ok(points)
    }

    pub(crate) fn prices(
        &mut self,
        connection: &Connection,
        wanted: &BTreeSet<(Symbol, NaiveDate)>,
    ) -> Result<Prices> {
        let unknown: BTreeSet<(Symbol, NaiveDate)> = wanted
            .iter()
            .filter(|key| !self.prices.contains_key(*key))
            .cloned()
            .collect();
        let mut fetched = daily_prices(connection, &unknown)?;
        for key in unknown {
            let price = fetched.remove(&key);
            self.prices.insert(key, price);
        }
        Ok(wanted
            .iter()
            .filter_map(|key| Some((key.clone(), self.prices.get(key).copied().flatten()?)))
            .collect())
    }
}

fn stored(
    connection: &Connection,
    symbols: &[Symbol],
    from: Timestamp,
    to: Timestamp,
) -> Result<BTreeMap<Symbol, Vec<Bar>>> {
    let mut grouped: BTreeMap<Symbol, Vec<Bar>> = BTreeMap::new();
    if symbols.is_empty() || from >= to {
        return Ok(grouped);
    }
    for bar in bars(
        connection,
        &BarQuery {
            timeframe: Timeframe::Day,
            symbols,
            from: Some(from),
            to,
            to_inclusive: false,
            latest: None,
        },
    )? {
        grouped.entry(bar.symbol.clone()).or_default().push(bar);
    }
    Ok(grouped)
}
