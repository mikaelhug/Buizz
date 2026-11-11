use buizz_core::api::SparkPoint;
use buizz_core::market::RiskFreeRate;
use buizz_core::order::Fill;
use buizz_core::performance::Metrics;
use buizz_core::run::EquityPoint;

pub struct MetricsInput<'a> {
    pub equity: &'a [EquityPoint],
    pub benchmark: &'a [SparkPoint],
    pub fills: &'a [Fill],
    pub risk_free: &'a [RiskFreeRate],
}

pub fn compute(_input: &MetricsInput<'_>) -> Metrics {
    unimplemented!()
}
