use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError};

use buizz_core::api::{
    BacktestGlance, GateCheck, PresetResearch, Research, RunSummary, Screening, SparkPoint, Trials,
    VariantResult,
};
use buizz_core::market::{RiskFreeRate, Symbol};
use buizz_core::performance::Metrics;
use buizz_core::run::{CostModel, Mode, Period, PeriodPreset, RunConfig, RunId, RunStatus};
use buizz_core::time::Timestamp;
use buizz_market::MarketStore;
use buizz_metrics::Screened;
use buizz_state::{RunFilter, StateStore};
use buizz_supervisor::BotSpec;
use chrono::NaiveDate;
use serde_json::{Map, Value};
use tokio::sync::OnceCell;

use crate::{Runner, RunnerError, report};

const SCREENED_STATUSES: [RunStatus; 5] = [
    RunStatus::Queued,
    RunStatus::Starting,
    RunStatus::Running,
    RunStatus::Completed,
    RunStatus::Failed,
];

type BenchmarkKey = (Symbol, NaiveDate, NaiveDate);

struct Figures {
    curve: Vec<SparkPoint>,
    risk_free: Vec<RiskFreeRate>,
    metrics: Metrics,
}

pub struct Benchmarks {
    market: MarketStore,
    state: StateStore,
    today: NaiveDate,
    figures: Mutex<HashMap<BenchmarkKey, Arc<OnceCell<Figures>>>>,
}

pub struct BotScreening {
    pub screening: Option<Screening>,
    pub backtest: Option<BacktestGlance>,
}

struct Gated {
    failed_checks: Vec<Vec<GateCheck>>,
    neighbourhood: Vec<Option<f64>>,
    plateau: Option<RunId>,
}

impl Benchmarks {
    async fn figures(&self, run: &RunSummary) -> Result<Arc<OnceCell<Figures>>, RunnerError> {
        let config = &run.config;
        let key = (
            config.benchmark.clone(),
            config.start,
            config.end.unwrap_or(self.today),
        );
        let cell = Arc::clone(
            self.figures
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(key)
                .or_default(),
        );
        cell.get_or_try_init(|| async {
            let curve = report::benchmark(&self.market, run, self.today).await?;
            let (metrics, risk_free) = report::benchmark_metrics(&self.market, run, &curve).await?;
            Ok::<_, RunnerError>(Figures {
                curve,
                risk_free,
                metrics,
            })
        })
        .await?;
        Ok(cell)
    }

    async fn metrics(&self, run: &RunSummary) -> Result<Metrics, RunnerError> {
        let cell = self.figures(run).await?;
        Ok(cell
            .get()
            .expect("benchmark figures are initialised")
            .metrics
            .clone())
    }

    async fn curve(&self, run: &RunSummary) -> Result<Vec<SparkPoint>, RunnerError> {
        let cell = self.figures(run).await?;
        Ok(cell
            .get()
            .expect("benchmark figures are initialised")
            .curve
            .clone())
    }

    async fn gate(&self, run: &RunSummary) -> Result<Vec<GateCheck>, RunnerError> {
        if !run.status.is_finished() {
            return Ok(buizz_metrics::gate(run, None));
        }
        let cell = self.figures(run).await?;
        let figures = cell.get().expect("benchmark figures are initialised");
        let equity = self.state.equity(run.id).await?;
        Ok(buizz_metrics::gate(
            run,
            Some(&Screened {
                equity: &equity,
                benchmark: &figures.curve,
                risk_free: &figures.risk_free,
            }),
        ))
    }
}

async fn gated(runs: &[RunSummary], benchmarks: &Benchmarks) -> Result<Gated, RunnerError> {
    let mut failed_checks = Vec::with_capacity(runs.len());
    for run in runs {
        failed_checks.push(benchmarks.gate(run).await?);
    }
    let sharpes: Vec<_> = runs
        .iter()
        .map(|run| {
            (
                &run.config.params,
                run.metrics.as_ref().and_then(|metrics| metrics.sharpe),
            )
        })
        .collect();
    let neighbourhood = buizz_metrics::neighbourhood_sharpes(&sharpes);
    let plateau = runs
        .iter()
        .zip(&failed_checks)
        .zip(&neighbourhood)
        .filter_map(|((run, failed), median)| failed.is_empty().then_some((run.id, (*median)?)))
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(run_id, _)| run_id);
    Ok(Gated {
        failed_checks,
        neighbourhood,
        plateau,
    })
}

fn featured<'a>(
    runs: &'a [RunSummary],
    plateau: Option<RunId>,
    defaults: &Map<String, Value>,
) -> Option<(&'a RunSummary, Metrics)> {
    let completed = || {
        runs.iter()
            .filter(|run| run.status == RunStatus::Completed)
            .filter_map(|run| Some((run, run.metrics.clone()?)))
    };
    completed()
        .find(|(run, _)| Some(run.id) == plateau)
        .or_else(|| completed().find(|(run, _)| &run.config.params == defaults))
        .or_else(|| completed().next())
}

fn latest_variants(
    runs: Vec<RunSummary>,
    standard: impl Fn(&RunConfig) -> bool,
) -> BTreeMap<PeriodPreset, Vec<RunSummary>> {
    let mut presets: BTreeMap<PeriodPreset, Vec<RunSummary>> = BTreeMap::new();
    for run in runs {
        let Some(to) = run.config.end.filter(|_| standard(&run.config)) else {
            continue;
        };
        let period = Period {
            from: run.config.start,
            to,
        };
        let Some(preset) = PeriodPreset::of(period, run.created_at.exchange_date()) else {
            continue;
        };
        let variants = presets.entry(preset).or_default();
        if !variants
            .iter()
            .any(|seen| seen.config.params == run.config.params)
        {
            variants.push(run);
        }
    }
    presets
}

fn gate_preset(presets: &BTreeMap<PeriodPreset, Vec<RunSummary>>) -> Option<PeriodPreset> {
    presets
        .iter()
        .rev()
        .find(|(_, runs)| runs.iter().any(|run| run.status.is_finished()))
        .map(|(preset, _)| *preset)
}

impl Runner {
    pub fn benchmarks(&self) -> Benchmarks {
        Benchmarks {
            market: self.inner.market.clone(),
            state: self.inner.state.clone(),
            today: Timestamp::now().exchange_date(),
            figures: Mutex::default(),
        }
    }

    pub(crate) fn standard(&self, config: &RunConfig, spec: &BotSpec) -> bool {
        config.cost == CostModel::default()
            && config.risk == spec.manifest.risk
            && config.start_cash == self.inner.config.start_cash
    }

    async fn screened_runs(
        &self,
        spec: &BotSpec,
    ) -> Result<BTreeMap<PeriodPreset, Vec<RunSummary>>, RunnerError> {
        let runs = self
            .inner
            .state
            .runs(RunFilter {
                bot: Some(spec.name.clone()),
                bot_hash: Some(spec.content_hash.clone()),
                mode: Some(Mode::Backtest),
                statuses: SCREENED_STATUSES.to_vec(),
                ..RunFilter::default()
            })
            .await?;
        Ok(latest_variants(runs, |config| self.standard(config, spec)))
    }

    pub async fn screening(
        &self,
        spec: &BotSpec,
        benchmarks: &Benchmarks,
    ) -> Result<BotScreening, RunnerError> {
        let presets = self.screened_runs(spec).await?;
        let Some(preset) = gate_preset(&presets) else {
            return Ok(BotScreening {
                screening: None,
                backtest: None,
            });
        };
        let runs = &presets[&preset];
        let gated = gated(runs, benchmarks).await?;
        let finished = || {
            runs.iter()
                .zip(&gated.failed_checks)
                .filter(|(run, _)| run.status.is_finished())
        };
        let screening = Screening {
            preset,
            variants: u32::try_from(finished().count()).unwrap_or(u32::MAX),
            passing: u32::try_from(finished().filter(|(_, failed)| failed.is_empty()).count())
                .unwrap_or(u32::MAX),
        };
        let backtest = match featured(runs, gated.plateau, &spec.manifest.params) {
            Some((run, metrics)) => Some(
                self.glance(run, metrics, preset, gated.plateau, benchmarks)
                    .await?,
            ),
            None => None,
        };
        Ok(BotScreening {
            screening: Some(screening),
            backtest,
        })
    }

    async fn glance(
        &self,
        run: &RunSummary,
        metrics: Metrics,
        preset: PeriodPreset,
        plateau: Option<RunId>,
        benchmarks: &Benchmarks,
    ) -> Result<BacktestGlance, RunnerError> {
        let equity = self.inner.state.equity(run.id).await?;
        let benchmark_curve = benchmarks.curve(run).await?;
        Ok(BacktestGlance {
            run_id: run.id,
            preset,
            plateau: plateau == Some(run.id),
            params: run.config.params.clone(),
            metrics,
            benchmark: benchmarks.metrics(run).await?,
            curve: buizz_metrics::backtest_curve(&equity, &benchmark_curve),
        })
    }

    pub async fn research(&self, bot: &str) -> Result<Research, RunnerError> {
        let spec = self.bot(bot).await?;
        let presets = self.screened_runs(&spec).await?;
        let trials = self.inner.state.trials(bot).await?;
        let benchmarks = self.benchmarks();
        let gate_preset = gate_preset(&presets);
        let mut researched = Vec::new();
        for (preset, runs) in presets.into_iter().rev() {
            researched.push(preset_research(preset, runs, trials, &benchmarks).await?);
        }
        Ok(Research {
            trials,
            gate_preset,
            presets: researched,
        })
    }
}

async fn preset_research(
    preset: PeriodPreset,
    runs: Vec<RunSummary>,
    trials: Trials,
    benchmarks: &Benchmarks,
) -> Result<PresetResearch, RunnerError> {
    let latest = match runs.first() {
        Some(run) => Some(benchmarks.metrics(run).await?),
        None => None,
    };
    let Gated {
        failed_checks,
        neighbourhood,
        plateau,
    } = gated(&runs, benchmarks).await?;
    let variants = runs
        .into_iter()
        .zip(failed_checks)
        .zip(neighbourhood)
        .map(
            |((run, failed_checks), neighbourhood_sharpe)| VariantResult {
                run_id: run.id,
                status: run.status,
                period: Period {
                    from: run.config.start,
                    to: run.config.end.unwrap_or(run.config.start),
                },
                params: run.config.params,
                metrics: run.metrics,
                badges: run.badges,
                failed_checks,
                neighbourhood_sharpe,
            },
        )
        .collect();
    Ok(PresetResearch {
        preset,
        benchmark_sharpe: latest.as_ref().and_then(|metrics| metrics.sharpe),
        benchmark_max_drawdown: latest.as_ref().and_then(|metrics| metrics.max_drawdown),
        luck_sharpe: latest
            .as_ref()
            .and_then(|metrics| buizz_metrics::luck_sharpe(metrics.sessions, trials.variants)),
        plateau,
        variants,
    })
}
