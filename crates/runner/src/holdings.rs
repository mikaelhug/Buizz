use std::collections::{BTreeMap, BTreeSet, HashMap};

use buizz_core::market::{FundamentalMetric, FundamentalPoint, PeriodType, Symbol};
use buizz_core::time::Timestamp;
use buizz_market::MarketStore;
use chrono::{Days, NaiveDate, NaiveTime};
use rust_decimal::prelude::ToPrimitive;

use crate::RunnerError;

const MAX_STALENESS_DAYS: i64 = 548;
const DOLLARS: &str = "USD";
const SHARES: &str = "shares";
const METRICS: [FundamentalMetric; 8] = [
    FundamentalMetric::Equity,
    FundamentalMetric::NetIncome,
    FundamentalMetric::SharesOutstanding,
    FundamentalMetric::LongTermDebtNoncurrent,
    FundamentalMetric::LongTermDebt,
    FundamentalMetric::ShortTermDebt,
    FundamentalMetric::ShortTermBorrowings,
    FundamentalMetric::CurrentLongTermDebt,
];

pub(crate) struct Company {
    pub equity: Option<f64>,
    pub roe: Option<f64>,
    pub debt_to_equity: Option<f64>,
    pub shares: Option<(f64, NaiveDate)>,
}

struct Facts<'a> {
    points: Vec<&'a FundamentalPoint>,
    on: NaiveDate,
}

impl Facts<'_> {
    fn fresh(&self, point: &FundamentalPoint) -> bool {
        point.period_end <= self.on && (self.on - point.period_end).num_days() <= MAX_STALENESS_DAYS
    }

    fn latest(&self, metric: FundamentalMetric, unit: &str) -> Option<(&FundamentalPoint, f64)> {
        self.points
            .iter()
            .filter(|point| {
                point.metric == metric
                    && point.unit == unit
                    && point.period_start.is_none()
                    && self.fresh(point)
            })
            .max_by_key(|point| (point.period_end, point.filed))
            .and_then(|point| Some((*point, point.value.to_f64()?)))
            .filter(|(_, value)| *value > 0.0)
    }

    fn on_day(&self, metric: FundamentalMetric, day: NaiveDate) -> Option<f64> {
        self.points
            .iter()
            .find(|point| {
                point.metric == metric
                    && point.unit == DOLLARS
                    && point.period_start.is_none()
                    && point.period_end == day
            })
            .and_then(|point| point.value.to_f64())
    }

    fn debt(&self, day: NaiveDate) -> f64 {
        let reported = |metric| self.on_day(metric, day);
        let borrowings = reported(FundamentalMetric::ShortTermBorrowings);
        let current = reported(FundamentalMetric::ShortTermDebt).unwrap_or_else(|| {
            borrowings.unwrap_or(0.0)
                + reported(FundamentalMetric::CurrentLongTermDebt).unwrap_or(0.0)
        });
        match (
            reported(FundamentalMetric::LongTermDebtNoncurrent),
            reported(FundamentalMetric::LongTermDebt),
        ) {
            (Some(noncurrent), _) => noncurrent + current,
            (None, Some(total)) => total + borrowings.unwrap_or(0.0),
            (None, None) => current,
        }
    }

    fn roe(&self) -> Option<f64> {
        let (end, income) = self
            .points
            .iter()
            .filter(|point| {
                point.metric == FundamentalMetric::NetIncome
                    && point.unit == DOLLARS
                    && point.period_type == PeriodType::Annual
                    && point.period_start.is_some()
                    && self.fresh(point)
            })
            .max_by_key(|point| point.period_end)
            .and_then(|point| Some((point.period_end, point.value.to_f64()?)))?;
        let book = self.on_day(FundamentalMetric::Equity, end)?;
        (book > 0.0).then(|| income / book)
    }

    fn company(&self) -> Company {
        let equity = self.latest(FundamentalMetric::Equity, DOLLARS);
        Company {
            equity: equity.map(|(_, equity)| equity),
            roe: self.roe(),
            debt_to_equity: equity.map(|(point, equity)| self.debt(point.period_end) / equity),
            shares: self
                .latest(FundamentalMetric::SharesOutstanding, SHARES)
                .map(|(point, count)| (count, point.filed)),
        }
    }
}

pub(crate) struct Fundamentals(HashMap<(NaiveDate, Symbol), Company>);

impl Fundamentals {
    pub async fn load(
        market: &MarketStore,
        observed: BTreeMap<NaiveDate, BTreeSet<Symbol>>,
    ) -> Result<Self, RunnerError> {
        let mut companies = HashMap::new();
        for (on, symbols) in observed {
            let Some(as_of) = on
                .checked_add_days(Days::new(1))
                .and_then(|next| Timestamp::at_exchange_time(next, NaiveTime::MIN))
            else {
                continue;
            };
            let points = market
                .fundamentals(
                    symbols.iter().cloned().collect(),
                    METRICS.to_vec(),
                    as_of,
                    None,
                )
                .await?;
            let mut by_symbol: HashMap<&Symbol, Vec<&FundamentalPoint>> = HashMap::new();
            for point in &points {
                by_symbol.entry(&point.symbol).or_default().push(point);
            }
            for symbol in symbols {
                let facts = Facts {
                    points: by_symbol.remove(&symbol).unwrap_or_default(),
                    on,
                };
                companies.insert((on, symbol), facts.company());
            }
        }
        Ok(Self(companies))
    }

    pub fn company(&self, on: NaiveDate, symbol: &Symbol) -> Option<&Company> {
        self.0.get(&(on, symbol.clone()))
    }
}
