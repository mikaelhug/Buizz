use std::collections::BTreeMap;

use buizz_core::study::AbnormalStats;

use crate::mean;
use crate::research::median;

pub fn luck_t(trials: u32) -> f64 {
    (2.0 * f64::from(trials.max(1)).ln()).sqrt()
}

pub fn abnormal_stats(observations: &[(i32, f64)], overlap_months: u32) -> AbnormalStats {
    if observations.is_empty() {
        return AbnormalStats::default();
    }
    let values: Vec<f64> = observations.iter().map(|(_, value)| *value).collect();
    let mut months: BTreeMap<i32, Vec<f64>> = BTreeMap::new();
    for (month, value) in observations {
        months.entry(*month).or_default().push(*value);
    }
    let monthly: BTreeMap<i32, f64> = months
        .into_iter()
        .map(|(month, values)| (month, mean(&values)))
        .collect();
    let hits = values.iter().filter(|value| **value > 0.0).count();
    AbnormalStats {
        mean: Some(mean(&values)),
        median: median(values.clone()),
        hit_rate: Some(hits as f64 / values.len() as f64),
        t_stat: newey_west_t(&monthly, overlap_months),
    }
}

fn newey_west_t(monthly: &BTreeMap<i32, f64>, lags: u32) -> Option<f64> {
    if monthly.len() < 2 {
        return None;
    }
    let count = monthly.len() as f64;
    let average = monthly.values().sum::<f64>() / count;
    let autocovariance = |gap: i32| {
        monthly
            .iter()
            .filter_map(|(month, value)| {
                monthly
                    .get(&(month - gap))
                    .map(|earlier| (value - average) * (earlier - average))
            })
            .sum::<f64>()
            / count
    };
    let weighted: f64 = (1..=lags)
        .map(|lag| {
            let weight = 1.0 - f64::from(lag) / f64::from(lags + 1);
            weight * autocovariance(i32::try_from(lag).unwrap_or(i32::MAX))
        })
        .sum();
    let variance = (autocovariance(0) + 2.0 * weighted) / count;
    (variance > 0.0).then(|| average / variance.sqrt())
}
