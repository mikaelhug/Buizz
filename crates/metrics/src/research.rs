use std::cmp::Ordering;

use buizz_core::api::{GateCheck, RunSummary};
use buizz_core::performance::Metrics;
use buizz_core::run::{RunBadge, RunStatus};
use serde_json::{Map, Value};

use crate::TRADING_DAYS;

const GATE_ROUND_TRIPS: u32 = 10;

fn beats(value: Option<f64>, against: Option<f64>, better: fn(f64, f64) -> bool) -> bool {
    match (value, against) {
        (Some(value), Some(against)) => better(value, against),
        (_, None) => true,
        (None, Some(_)) => false,
    }
}

pub fn gate(run: &RunSummary, benchmark: Option<&Metrics>) -> Vec<GateCheck> {
    if !run.status.is_finished() {
        return vec![GateCheck::NotCompleted];
    }
    let metrics = run.metrics.as_ref();
    let sharpe = metrics.and_then(|metrics| metrics.sharpe);
    let drawdown = metrics.and_then(|metrics| metrics.max_drawdown);
    let dominated = !(beats(sharpe, benchmark.and_then(|spy| spy.sharpe), |run, spy| {
        run >= spy
    }) || beats(
        drawdown,
        benchmark.and_then(|spy| spy.max_drawdown),
        |run, spy| run < spy,
    ));
    let stopped = run.badges.contains(&RunBadge::DrawdownStop);
    [
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
        (GateCheck::DominatedByBenchmark, dominated),
    ]
    .into_iter()
    .filter_map(|(check, failed)| failed.then_some(check))
    .collect()
}

pub fn luck_sharpe(sessions: u32, trials: u32) -> Option<f64> {
    (sessions >= 2 && trials >= 1).then(|| {
        (TRADING_DAYS / f64::from(sessions)).sqrt() * (2.0 * f64::from(trials).ln()).sqrt()
    })
}

fn median(mut values: Vec<f64>) -> Option<f64> {
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
