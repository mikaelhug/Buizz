use crate::{MIN_DAILY_VOLATILITY, TRADING_DAYS, beta, mean, sample_covariance, sharpe};

pub(crate) struct Active {
    pub(crate) alpha: f64,
    pub(crate) beta: f64,
    pub(crate) residual_volatility: Option<f64>,
    pub(crate) appraisal_ratio: Option<f64>,
    pub(crate) alpha_t: Option<f64>,
    pub(crate) overlay_sharpe: Option<f64>,
}

impl Active {
    pub(crate) fn regress(excess: &[f64], benchmark: &[f64]) -> Option<Self> {
        let beta = beta(excess, benchmark)?;
        let daily_alpha = mean(excess) - beta * mean(benchmark);
        let residuals: Vec<f64> = excess
            .iter()
            .zip(benchmark)
            .map(|(own, theirs)| own - daily_alpha - beta * theirs)
            .collect();
        let freedom = residuals.len().checked_sub(2).filter(|left| *left > 0);
        let residual = freedom.map(|left| {
            (residuals.iter().map(|value| value * value).sum::<f64>() / left as f64).sqrt()
        });
        let spread = residual.filter(|stdev| *stdev > MIN_DAILY_VOLATILITY);
        let appraisal_ratio = spread.map(|stdev| daily_alpha / stdev * TRADING_DAYS.sqrt());
        Some(Self {
            alpha: daily_alpha * TRADING_DAYS,
            beta,
            residual_volatility: residual.map(|stdev| stdev * TRADING_DAYS.sqrt()),
            appraisal_ratio,
            alpha_t: spread.and_then(|_| newey_west_t(daily_alpha, benchmark, &residuals)),
            overlay_sharpe: sharpe(benchmark)
                .filter(|sharpe| *sharpe > 0.0)
                .zip(appraisal_ratio)
                .map(|(sharpe, ratio)| sharpe.hypot(ratio.max(0.0))),
        })
    }
}

fn newey_west_t(daily_alpha: f64, benchmark: &[f64], residuals: &[f64]) -> Option<f64> {
    let sessions = residuals.len() as f64;
    let centre = mean(benchmark);
    let spread = sample_covariance(benchmark, benchmark)? * (sessions - 1.0);
    let influence: Vec<f64> = benchmark
        .iter()
        .zip(residuals)
        .map(|(theirs, residual)| (1.0 / sessions - centre * (theirs - centre) / spread) * residual)
        .collect();
    let lags = (4.0 * (sessions / 100.0).powf(2.0 / 9.0)).floor() as usize;
    let autocovariance = |lag: usize| -> f64 {
        influence[lag..]
            .iter()
            .zip(&influence)
            .map(|(later, earlier)| later * earlier)
            .sum()
    };
    let long_run: f64 = autocovariance(0)
        + (1..=lags)
            .map(|lag| 2.0 * (1.0 - lag as f64 / (lags + 1) as f64) * autocovariance(lag))
            .sum::<f64>();
    let variance = long_run * sessions / (sessions - 2.0);
    (variance > 0.0).then(|| daily_alpha / variance.sqrt())
}
