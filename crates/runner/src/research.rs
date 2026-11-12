use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError};

use buizz_core::api::{PresetResearch, Research, RunSummary, Screening, Trials, VariantResult};
use buizz_core::market::Symbol;
use buizz_core::performance::Metrics;
use buizz_core::run::{CostModel, Mode, Period, PeriodPreset, RunStatus};
use buizz_core::time::Timestamp;
use buizz_market::MarketStore;
use buizz_state::RunFilter;
use chrono::NaiveDate;
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

pub struct Benchmarks {
    market: MarketStore,
    today: NaiveDate,
    figures: Mutex<HashMap<BenchmarkKey, Arc<OnceCell<Metrics>>>>,
}

impl Benchmarks {
    async fn metrics(&self, run: &RunSummary) -> Result<Metrics, RunnerError> {
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
        let metrics = cell
            .get_or_try_init(|| async {
                let curve = report::benchmark(&self.market, run, self.today).await?;
                report::benchmark_metrics(&self.market, run, &curve).await
            })
            .await?;
        Ok(metrics.clone())
    }
}

pub(crate) fn screening_period(run: &RunSummary) -> Option<PeriodPreset> {
    let period = Period {
        from: run.config.start,
        to: run.config.end?,
    };
    (run.config.cost == CostModel::default())
        .then(|| PeriodPreset::of(period, run.created_at.exchange_date()))
        .flatten()
}

fn latest_variants(runs: Vec<RunSummary>) -> BTreeMap<PeriodPreset, Vec<RunSummary>> {
    let mut presets: BTreeMap<PeriodPreset, Vec<RunSummary>> = BTreeMap::new();
    for run in runs {
        let Some(preset) = screening_period(&run) else {
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
            today: Timestamp::now().exchange_date(),
            figures: Mutex::default(),
        }
    }

    async fn screened_runs(
        &self,
        bot: &str,
        bot_hash: &str,
    ) -> Result<BTreeMap<PeriodPreset, Vec<RunSummary>>, RunnerError> {
        let runs = self
            .inner
            .state
            .runs(RunFilter {
                bot: Some(bot.to_owned()),
                bot_hash: Some(bot_hash.to_owned()),
                mode: Some(Mode::Backtest),
                statuses: SCREENED_STATUSES.to_vec(),
                ..RunFilter::default()
            })
            .await?;
        Ok(latest_variants(runs))
    }

    pub async fn screening(
        &self,
        bot: &str,
        bot_hash: &str,
        benchmarks: &Benchmarks,
    ) -> Result<Option<Screening>, RunnerError> {
        let presets = self.screened_runs(bot, bot_hash).await?;
        let Some(preset) = gate_preset(&presets) else {
            return Ok(None);
        };
        let (mut variants, mut passing) = (0, 0);
        for run in presets[&preset]
            .iter()
            .filter(|run| run.status.is_finished())
        {
            let benchmark = benchmarks.metrics(run).await?;
            variants += 1;
            if buizz_metrics::gate(run, Some(&benchmark)).is_empty() {
                passing += 1;
            }
        }
        Ok(Some(Screening {
            preset,
            variants,
            passing,
        }))
    }

    pub async fn research(&self, bot: &str) -> Result<Research, RunnerError> {
        let spec = self.bot(bot).await?;
        let presets = self.screened_runs(bot, &spec.content_hash).await?;
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
    let mut failed_checks = Vec::with_capacity(runs.len());
    for run in &runs {
        let benchmark = if run.status.is_finished() {
            Some(benchmarks.metrics(run).await?)
        } else {
            None
        };
        failed_checks.push(buizz_metrics::gate(run, benchmark.as_ref()));
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
