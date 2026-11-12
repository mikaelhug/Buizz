use std::collections::{BTreeMap, HashMap};

use buizz_core::api::SparkPoint;
use buizz_core::ledger::{LedgerEntry, LedgerKind};
use buizz_core::market::{RiskFreeRate, Symbol};
use buizz_core::money::round_cash;
use buizz_core::order::OrderId;
use buizz_core::performance::{Metrics, RoundTrip};
use buizz_core::run::{CostModel, EquityPoint};
use buizz_core::time::Timestamp;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

pub use crate::live::{LiveInput, Source, backtest_curve, live, live_summary};
pub use crate::research::{gate, luck_sharpe, neighbourhood_sharpes};

mod envelope;
mod live;
mod research;

const TRADING_DAYS: f64 = CostModel::TRADING_DAYS_PER_YEAR as f64;
const MIN_SPREAD: f64 = f64::EPSILON;

pub struct MetricsInput<'a> {
    pub start_cash: Decimal,
    pub equity: &'a [EquityPoint],
    pub benchmark: &'a [SparkPoint],
    pub ledger: &'a [LedgerEntry],
    pub risk_free: &'a [RiskFreeRate],
}

pub fn compute(input: &MetricsInput<'_>) -> Metrics {
    let closes: Vec<f64> = input
        .equity
        .iter()
        .map(|point| point.equity.to_f64().unwrap_or(f64::NAN))
        .collect();
    let values: Vec<f64> = (!closes.is_empty())
        .then(|| input.start_cash.to_f64().unwrap_or(f64::NAN))
        .into_iter()
        .chain(closes.iter().copied())
        .collect();
    let returns = simple_returns(&values);
    let excess = returns
        .as_deref()
        .and_then(|returns| excess_returns(input, returns));
    let cagr = cagr(input.start_cash, input.equity);
    let max_drawdown = max_drawdown(&values);
    let trips = round_trips(input.ledger);
    let (alpha, beta) = returns
        .as_deref()
        .and_then(|returns| alpha_beta(input, returns))
        .unzip();

    Metrics {
        total_return: final_ratio(input.start_cash, input.equity).map(|ratio| ratio - 1.0),
        cagr,
        volatility: returns
            .as_deref()
            .and_then(sample_stdev)
            .map(|stdev| stdev * TRADING_DAYS.sqrt()),
        sharpe: excess.as_deref().and_then(sharpe),
        sortino: excess.as_deref().and_then(sortino),
        max_drawdown,
        calmar: cagr
            .zip(max_drawdown)
            .and_then(|(cagr, drawdown)| (drawdown > 0.0).then(|| cagr / drawdown)),
        win_rate: win_rate(&trips),
        profit_factor: profit_factor(&trips),
        exposure: exposure(input.equity),
        turnover: turnover(input.ledger, &closes),
        alpha,
        beta,
        trades: u32::try_from(trips.len()).unwrap_or(u32::MAX),
        sessions: u32::try_from(input.equity.len()).unwrap_or(u32::MAX),
    }
}

fn simple_returns(values: &[f64]) -> Option<Vec<f64>> {
    values
        .windows(2)
        .map(|pair| (pair[0] > 0.0 && pair[1].is_finite()).then(|| pair[1] / pair[0] - 1.0))
        .collect()
}

fn excess_returns(input: &MetricsInput<'_>, returns: &[f64]) -> Option<Vec<f64>> {
    input
        .equity
        .iter()
        .zip(returns)
        .map(|(point, ret)| daily_risk_free(input.risk_free, point).map(|rf| ret - rf))
        .collect()
}

fn daily_risk_free(rates: &[RiskFreeRate], point: &EquityPoint) -> Option<f64> {
    let date = point.at.exchange_date();
    rates
        .iter()
        .filter(|rate| rate.date <= date)
        .max_by_key(|rate| rate.date)
        .and_then(|rate| (rate.percent / Decimal::from(100)).to_f64())
        .map(|annual| annual / TRADING_DAYS)
}

fn cagr(start_cash: Decimal, equity: &[EquityPoint]) -> Option<f64> {
    let periods = equity.len() as f64;
    if periods < TRADING_DAYS {
        return None;
    }
    final_ratio(start_cash, equity).map(|ratio| ratio.max(0.0).powf(TRADING_DAYS / periods) - 1.0)
}

fn final_ratio(start_cash: Decimal, equity: &[EquityPoint]) -> Option<f64> {
    let last = equity.last()?;
    (start_cash > Decimal::ZERO)
        .then(|| (last.equity / start_cash).to_f64())
        .flatten()
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

fn sample_stdev(values: &[f64]) -> Option<f64> {
    sample_covariance(values, values).map(f64::sqrt)
}

fn sample_covariance(left: &[f64], right: &[f64]) -> Option<f64> {
    if left.len() < 2 || left.len() != right.len() {
        return None;
    }
    let (left_mean, right_mean) = (mean(left), mean(right));
    let sum: f64 = left
        .iter()
        .zip(right)
        .map(|(l, r)| (l - left_mean) * (r - right_mean))
        .sum();
    Some(sum / (left.len() - 1) as f64)
}

fn sharpe(excess: &[f64]) -> Option<f64> {
    let stdev = sample_stdev(excess).filter(|stdev| *stdev > MIN_SPREAD)?;
    Some(mean(excess) / stdev * TRADING_DAYS.sqrt())
}

fn sortino(excess: &[f64]) -> Option<f64> {
    if excess.len() < 2 {
        return None;
    }
    let shortfall: f64 = excess.iter().map(|value| value.min(0.0).powi(2)).sum();
    let downside = (shortfall / excess.len() as f64).sqrt();
    (downside > MIN_SPREAD).then(|| mean(excess) / downside * TRADING_DAYS.sqrt())
}

fn max_drawdown(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut peak = f64::MIN;
    Some(values.iter().fold(0.0, |worst, value| {
        peak = peak.max(*value);
        if peak > 0.0 {
            worst.max((peak - value) / peak)
        } else {
            worst
        }
    }))
}

fn exposure(equity: &[EquityPoint]) -> Option<f64> {
    let ratios: Vec<f64> = equity
        .iter()
        .filter(|point| point.equity > Decimal::ZERO)
        .filter_map(|point| (point.gross_exposure / point.equity).to_f64())
        .collect();
    (!ratios.is_empty()).then(|| mean(&ratios))
}

fn turnover(ledger: &[LedgerEntry], equity: &[f64]) -> Option<f64> {
    let traded: Decimal = ledger
        .iter()
        .filter_map(|entry| match &entry.kind {
            LedgerKind::Fill { qty, price, .. } | LedgerKind::Liquidation { qty, price, .. } => {
                Some(round_cash(Decimal::from(*qty) * *price).abs())
            }
            _ => None,
        })
        .sum();
    if equity.is_empty() || mean(equity) <= 0.0 {
        return None;
    }
    traded.to_f64().map(|traded| traded / mean(equity))
}

struct OpenTrip {
    qty: i64,
    peak: i64,
    pnl: Decimal,
    opened_at: Timestamp,
    opened_by: Option<OrderId>,
}

impl OpenTrip {
    fn new(qty: i64, pnl: Decimal, opened_at: Timestamp, opened_by: Option<OrderId>) -> Self {
        Self {
            qty,
            peak: qty,
            pnl,
            opened_at,
            opened_by,
        }
    }

    fn close(
        &self,
        symbol: &Symbol,
        pnl: Decimal,
        closed_at: Timestamp,
        closed_by: Option<OrderId>,
    ) -> RoundTrip {
        RoundTrip {
            symbol: symbol.clone(),
            qty: self.peak,
            opened_at: self.opened_at,
            closed_at,
            opened_by: self.opened_by,
            closed_by,
            pnl,
        }
    }
}

pub fn round_trips(ledger: &[LedgerEntry]) -> Vec<RoundTrip> {
    let mut open: HashMap<&Symbol, OpenTrip> = HashMap::new();
    let mut closed = Vec::new();
    for entry in ledger {
        let Some(symbol) = entry.kind.symbol() else {
            continue;
        };
        let order = match &entry.kind {
            LedgerKind::Fill { order_id, .. } => Some(*order_id),
            _ => None,
        };
        let trip = open
            .entry(symbol)
            .or_insert_with(|| OpenTrip::new(0, Decimal::ZERO, entry.at, order));
        let cash = entry.kind.cash_delta();
        let before = trip.qty;
        let after = match &entry.kind {
            LedgerKind::Fill { qty, .. } | LedgerKind::Liquidation { qty, .. } => before + qty,
            LedgerKind::Split { qty_after, .. } => *qty_after,
            _ => before,
        };
        if before.signum() * after.signum() < 0 {
            let closing =
                cash * Decimal::from(before.abs()) / Decimal::from((after - before).abs());
            closed.push(trip.close(symbol, trip.pnl + closing, entry.at, order));
            *trip = OpenTrip::new(after, cash - closing, entry.at, order);
            continue;
        }
        if before == 0 {
            trip.opened_at = entry.at;
            trip.opened_by = order;
        }
        trip.qty = after;
        if after.abs() > trip.peak.abs() {
            trip.peak = after;
        }
        trip.pnl += cash;
        if before != 0 && after == 0 {
            closed.push(trip.close(symbol, trip.pnl, entry.at, order));
        }
        if after == 0 {
            open.remove(symbol);
        }
    }
    closed
}

fn win_rate(trips: &[RoundTrip]) -> Option<f64> {
    let wins = trips.iter().filter(|trip| trip.pnl > Decimal::ZERO).count();
    (!trips.is_empty()).then(|| wins as f64 / trips.len() as f64)
}

fn profit_factor(trips: &[RoundTrip]) -> Option<f64> {
    let gains: Decimal = trips
        .iter()
        .map(|trip| trip.pnl)
        .filter(|pnl| *pnl > Decimal::ZERO)
        .sum();
    let losses: Decimal = trips
        .iter()
        .map(|trip| -trip.pnl)
        .filter(|loss| *loss > Decimal::ZERO)
        .sum();
    (losses > Decimal::ZERO)
        .then(|| (gains / losses).to_f64())
        .flatten()
}

fn beta(returns: &[f64], benchmark: &[f64]) -> Option<f64> {
    let variance = sample_covariance(benchmark, benchmark)?;
    if variance <= MIN_SPREAD * MIN_SPREAD {
        return None;
    }
    Some(sample_covariance(returns, benchmark)? / variance)
}

fn alpha_beta(input: &MetricsInput<'_>, returns: &[f64]) -> Option<(f64, f64)> {
    let benchmark = benchmark_returns(input)?;
    let returns = returns.get(returns.len().checked_sub(benchmark.len())?..)?;
    let beta = beta(returns, &benchmark)?;
    let alpha = (mean(returns) - beta * mean(&benchmark)) * TRADING_DAYS;
    Some((alpha, beta))
}

fn benchmark_returns(input: &MetricsInput<'_>) -> Option<Vec<f64>> {
    let by_time: BTreeMap<Timestamp, f64> = input
        .benchmark
        .iter()
        .map(|point| (point.at, point.equity.to_f64().unwrap_or(f64::NAN)))
        .collect();
    let first = input.equity.first()?;
    let base = by_time
        .range(..first.at)
        .next_back()
        .map(|(_, value)| *value);
    let values: Vec<f64> = base
        .map(Some)
        .into_iter()
        .chain(
            input
                .equity
                .iter()
                .map(|point| by_time.get(&point.at).copied()),
        )
        .collect::<Option<_>>()?;
    simple_returns(&values)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use chrono::NaiveDate;

    use super::*;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
    }

    fn risk_free() -> Vec<RiskFreeRate> {
        fs::read_to_string(fixtures().join("market/rates.csv"))
            .unwrap()
            .lines()
            .skip(1)
            .map(|line| {
                let (date, percent) = line.split_once(',').unwrap();
                RiskFreeRate {
                    date: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
                    percent: percent.parse().unwrap(),
                }
            })
            .collect()
    }

    #[test]
    fn red_green_metrics_match_the_hand_computed_values() {
        let text =
            fs::read_to_string(fixtures().join("scenarios/red_green.expected.json")).unwrap();
        let scenario: serde_json::Value = serde_json::from_str(&text).unwrap();
        let equity: Vec<EquityPoint> = serde_json::from_value(scenario["equity"].clone()).unwrap();
        let benchmark: Vec<SparkPoint> =
            serde_json::from_value(scenario["benchmark"].clone()).unwrap();
        let ledger: Vec<LedgerEntry> = serde_json::from_value(scenario["ledger"].clone()).unwrap();
        let risk_free = risk_free();
        let start_cash: Decimal = scenario["config"]["start_cash"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();

        let actual = compute(&MetricsInput {
            start_cash,
            equity: &equity,
            benchmark: &benchmark,
            ledger: &ledger,
            risk_free: &risk_free,
        });

        let expected: Metrics = serde_json::from_value(scenario["metrics"].clone()).unwrap();
        let tolerance: f64 = serde_json::from_value(scenario["metrics_tolerance"].clone()).unwrap();
        let close = |name: &str, actual: Option<f64>, expected: Option<f64>| {
            let (actual, expected) = (actual.unwrap(), expected.unwrap());
            assert!(
                (actual - expected).abs() <= tolerance,
                "{name}: {actual} vs {expected}"
            );
        };
        close("total_return", actual.total_return, expected.total_return);
        close("volatility", actual.volatility, expected.volatility);
        close("sharpe", actual.sharpe, expected.sharpe);
        close("sortino", actual.sortino, expected.sortino);
        close("max_drawdown", actual.max_drawdown, expected.max_drawdown);
        assert_eq!((actual.cagr, actual.calmar), (None, None));
        close("win_rate", actual.win_rate, expected.win_rate);
        close(
            "profit_factor",
            actual.profit_factor,
            expected.profit_factor,
        );
        close("exposure", actual.exposure, expected.exposure);
        close("turnover", actual.turnover, expected.turnover);
        close("alpha", actual.alpha, expected.alpha);
        close("beta", actual.beta, expected.beta);
        assert_eq!(actual.trades, expected.trades);
        assert_eq!(actual.sessions, expected.sessions);
    }

    #[test]
    fn missing_data_stays_absent() {
        let metrics = compute(&MetricsInput {
            start_cash: Decimal::from(1000),
            equity: &[],
            benchmark: &[],
            ledger: &[],
            risk_free: &[],
        });
        assert_eq!(metrics, Metrics::default());
    }

    #[test]
    fn a_flip_closes_one_round_trip_and_a_split_to_zero_closes_another() {
        let entry = |seq, kind| LedgerEntry {
            seq,
            at: Timestamp::from_datetime(chrono::DateTime::UNIX_EPOCH).unwrap(),
            kind,
        };
        let fill = |symbol: &str, qty: i64, price: i64| LedgerKind::Fill {
            order_id: buizz_core::order::OrderId(1),
            symbol: Symbol::new(symbol),
            qty,
            price: Decimal::from(price),
            commission: Decimal::ZERO,
            feed: buizz_core::market::Feed::Sip,
        };
        let ledger = [
            entry(1, fill("AAA", 100, 10)),
            entry(2, fill("AAA", -150, 12)),
            entry(3, fill("AAA", 50, 11)),
            entry(4, fill("BBB", 3, 10)),
            entry(
                5,
                LedgerKind::Split {
                    symbol: Symbol::new("BBB"),
                    old_rate: Decimal::from(10),
                    new_rate: Decimal::ONE,
                    qty_before: 3,
                    qty_after: 0,
                    cash_in_lieu: Decimal::from(25),
                },
            ),
        ];
        let trips = round_trips(&ledger);
        assert_eq!(
            trips.iter().map(|trip| trip.pnl).collect::<Vec<_>>(),
            [Decimal::from(200), Decimal::from(50), Decimal::from(-5)]
        );
        assert_eq!(
            trips.iter().map(|trip| trip.qty).collect::<Vec<_>>(),
            [100, -50, 3]
        );
    }
}
