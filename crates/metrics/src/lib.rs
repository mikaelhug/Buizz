use buizz_core::api::SparkPoint;
use buizz_core::ledger::LedgerEntry;
use buizz_core::market::RiskFreeRate;
use buizz_core::performance::Metrics;
use buizz_core::run::EquityPoint;
use rust_decimal::Decimal;

pub struct MetricsInput<'a> {
    pub start_cash: Decimal,
    pub equity: &'a [EquityPoint],
    pub benchmark: &'a [SparkPoint],
    pub ledger: &'a [LedgerEntry],
    pub risk_free: &'a [RiskFreeRate],
}

pub fn compute(_input: &MetricsInput<'_>) -> Metrics {
    unimplemented!()
}
