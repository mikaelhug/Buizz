use crate::{TRADING_DAYS, max_drawdown, sample_stdev, simple_returns};

const EXCESS_PERCENTILE: f64 = 0.05;
const DRAWDOWN_PERCENTILE: f64 = 0.95;

pub(crate) struct Backtest {
    pub(crate) held: Vec<f64>,
    pub(crate) index: Vec<f64>,
}

impl Backtest {
    pub(crate) fn sessions(&self) -> usize {
        self.held.len().saturating_sub(1)
    }

    pub(crate) fn windows(&self, age: usize) -> usize {
        self.held.len().saturating_sub(age)
    }

    fn starts(&self, age: usize) -> std::ops::Range<usize> {
        0..self.windows(age)
    }

    pub(crate) fn excess_floor(&self, age: usize) -> Option<f64> {
        let growth = |values: &[f64], start: usize| values[start + age] / values[start] - 1.0;
        percentile(
            self.starts(age)
                .map(|start| growth(&self.held, start) - growth(&self.index, start))
                .collect(),
            EXCESS_PERCENTILE,
        )
    }

    pub(crate) fn drawdown_ceiling(&self, age: usize) -> Option<f64> {
        percentile(
            self.starts(age)
                .filter_map(|start| max_drawdown(&self.held[start..=start + age]))
                .collect(),
            DRAWDOWN_PERCENTILE,
        )
    }

    pub(crate) fn volatility(&self) -> Option<f64> {
        sample_stdev(&simple_returns(&self.held)?).map(|stdev| stdev * TRADING_DAYS.sqrt())
    }
}

fn percentile(mut values: Vec<f64>, rank: f64) -> Option<f64> {
    if values.iter().any(|value| !value.is_finite()) {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let position = (values.len().checked_sub(1)? as f64) * rank;
    let below = position.floor() as usize;
    let above = position.ceil() as usize;
    Some(values[below] + (position - below as f64) * (values[above] - values[below]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: Option<f64>, expected: f64) {
        let actual = actual.unwrap();
        assert!((actual - expected).abs() < 1e-12, "{actual} vs {expected}");
    }

    #[test]
    fn the_envelope_takes_percentiles_over_every_rolling_window_of_the_live_age() {
        let backtest = Backtest {
            held: vec![100.0, 120.0, 114.0, 96.0, 120.0, 108.0],
            index: vec![100.0, 105.0, 105.0, 105.0, 105.0, 105.0],
        };
        assert_eq!(backtest.sessions(), 5);
        assert_eq!(backtest.windows(2), 4);

        close(backtest.excess_floor(2), -0.2 + 0.15 * (6.0 / 114.0 + 0.2));
        close(
            backtest.drawdown_ceiling(2),
            18.0 / 114.0 + 0.85 * (0.2 - 18.0 / 114.0),
        );
        close(backtest.excess_floor(0), 0.0);
        close(backtest.excess_floor(5), 0.08 - 0.05);
        assert_eq!(backtest.excess_floor(6), None);
        assert_eq!(backtest.drawdown_ceiling(6), None);
    }
}
