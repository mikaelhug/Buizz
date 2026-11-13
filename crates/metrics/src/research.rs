use std::cmp::Ordering;

use buizz_core::api::{GateCheck, RunSummary, SparkPoint};
use buizz_core::market::RiskFreeRate;
use buizz_core::run::{EquityPoint, RunBadge, RunStatus};
use rust_decimal::prelude::ToPrimitive;
use serde_json::{Map, Value};

use crate::{
    MIN_DAILY_VOLATILITY, TRADING_DAYS, benchmark_returns, daily_risk_free, luck_t, max_drawdown,
    sample_stdev, simple_returns,
};

const GATE_ROUND_TRIPS: u32 = 10;

const SUB_PERIODS: usize = 3;
const AHEAD_SUB_PERIODS: usize = 2;

pub struct Screened<'a> {
    pub equity: &'a [EquityPoint],
    pub benchmark: &'a [SparkPoint],
    pub risk_free: &'a [RiskFreeRate],
}

struct Matched {
    returns: Vec<f64>,
    benchmark: Vec<f64>,
}

impl Matched {
    fn of(run: &RunSummary, screened: &Screened<'_>) -> Option<Self> {
        let config = &run.config;
        let values: Vec<f64> = std::iter::once(config.start_cash)
            .chain(screened.equity.iter().map(|point| point.equity))
            .map(|value| value.to_f64().unwrap_or(f64::NAN))
            .collect();
        let returns = simple_returns(&values)?;
        let benchmark = benchmark_returns(screened.benchmark, screened.equity)?;
        if returns.len() != benchmark.len() {
            return None;
        }
        let volatility = sample_stdev(&returns).filter(|stdev| *stdev > MIN_DAILY_VOLATILITY)?;
        let scale = sample_stdev(&benchmark)? / volatility;
        let spread = config.cost.financing_spread.to_f64()? / TRADING_DAYS;
        let returns = screened
            .equity
            .iter()
            .zip(&returns)
            .map(|(point, ret)| {
                let risk_free = daily_risk_free(screened.risk_free, point)?;
                Some(risk_free + scale * (ret - risk_free) - (scale - 1.0).max(0.0) * spread)
            })
            .collect::<Option<Vec<f64>>>()?;
        Some(Self { returns, benchmark })
    }

    fn sub_period_edges(&self) -> Vec<f64> {
        let length = self.returns.len();
        (0..SUB_PERIODS)
            .map(|part| part * length / SUB_PERIODS..(part + 1) * length / SUB_PERIODS)
            .filter(|range| !range.is_empty())
            .map(|range| growth(&self.returns[range.clone()]) - growth(&self.benchmark[range]))
            .collect()
    }
}

fn growth(returns: &[f64]) -> f64 {
    returns.iter().map(|ret| 1.0 + ret).product()
}

fn annualized(returns: &[f64]) -> f64 {
    growth(returns)
        .max(0.0)
        .powf(TRADING_DAYS / returns.len() as f64)
        - 1.0
}

fn path(returns: &[f64]) -> Vec<f64> {
    std::iter::once(1.0)
        .chain(returns.iter().scan(1.0, |value, ret| {
            *value *= 1.0 + ret;
            Some(*value)
        }))
        .collect()
}

pub struct Screen {
    pub failed_checks: Vec<GateCheck>,
    pub sub_periods: Vec<f64>,
}

pub fn screen(run: &RunSummary, screened: Option<&Screened<'_>>) -> Screen {
    if !run.status.is_finished() {
        return Screen {
            failed_checks: vec![GateCheck::NotCompleted],
            sub_periods: Vec::new(),
        };
    }
    let metrics = run.metrics.as_ref();
    let sharpe = metrics.and_then(|metrics| metrics.sharpe);
    let matched = screened.and_then(|screened| Matched::of(run, screened));
    let sub_periods = matched
        .as_ref()
        .map(Matched::sub_period_edges)
        .unwrap_or_default();
    let (trails, deeper, inconsistent) = match &matched {
        Some(matched) => (
            annualized(&matched.returns) <= annualized(&matched.benchmark),
            max_drawdown(&path(&matched.returns)) > max_drawdown(&path(&matched.benchmark)),
            sub_periods.iter().filter(|edge| **edge > 0.0).count() < AHEAD_SUB_PERIODS,
        ),
        None => (true, true, true),
    };
    let stopped = run.badges.contains(&RunBadge::DrawdownStop);
    let checks = [
        (
            GateCheck::NotCompleted,
            !stopped && run.status != RunStatus::Completed,
        ),
        (GateCheck::DrawdownStop, stopped),
        (
            GateCheck::DataWarnings,
            run.badges.contains(&RunBadge::DataWarnings),
        ),
        (
            GateCheck::FewTrades,
            metrics.is_none_or(|metrics| metrics.trades < GATE_ROUND_TRIPS),
        ),
        (
            GateCheck::NoEdge,
            !sharpe.is_some_and(|sharpe| sharpe > 0.0),
        ),
        (GateCheck::TrailsBenchmark, trails),
        (GateCheck::DeeperDrawdown, deeper),
        (GateCheck::InconsistentEdge, inconsistent),
    ];
    Screen {
        failed_checks: checks
            .into_iter()
            .filter_map(|(check, failed)| failed.then_some(check))
            .collect(),
        sub_periods,
    }
}

pub fn luck_sharpe(sessions: u32, trials: u32) -> Option<f64> {
    (sessions >= 2 && trials >= 1)
        .then(|| (TRADING_DAYS / f64::from(sessions)).sqrt() * luck_t(trials))
}

pub(crate) fn median(mut values: Vec<f64>) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    match values.len() {
        0 => None,
        length if length % 2 == 1 => Some(values[middle]),
        _ => Some((values[middle - 1] + values[middle]) / 2.0),
    }
}

fn compare(left: &Value, right: &Value) -> Ordering {
    match (left.as_f64(), right.as_f64()) {
        (Some(left), Some(right)) => left.total_cmp(&right),
        _ => left.to_string().cmp(&right.to_string()),
    }
}

pub fn neighbourhood_sharpes(variants: &[(&Map<String, Value>, Option<f64>)]) -> Vec<Option<f64>> {
    let mut axes: Vec<(&String, Vec<&Value>)> = Vec::new();
    for (params, _) in variants {
        for (name, value) in params.iter() {
            match axes.iter_mut().find(|(axis, _)| *axis == name) {
                Some((_, values)) if !values.contains(&value) => values.push(value),
                Some(_) => {}
                None => axes.push((name, vec![value])),
            }
        }
    }
    for (_, values) in &mut axes {
        values.sort_by(|left, right| compare(left, right));
    }
    let positions: Vec<Vec<Option<usize>>> = variants
        .iter()
        .map(|(params, _)| {
            axes.iter()
                .map(|(name, values)| {
                    let value = params.get(name.as_str())?;
                    values.iter().position(|known| *known == value)
                })
                .collect()
        })
        .collect();
    variants
        .iter()
        .zip(&positions)
        .map(|((_, sharpe), centre)| {
            sharpe.as_ref()?;
            let around = variants
                .iter()
                .zip(&positions)
                .filter(|(_, other)| {
                    centre.iter().zip(other.iter()).all(|pair| match pair {
                        (Some(at), Some(near)) => at.abs_diff(*near) <= 1,
                        (at, near) => at == near,
                    })
                })
                .filter_map(|((_, sharpe), _)| *sharpe)
                .collect();
            median(around)
        })
        .collect()
}
