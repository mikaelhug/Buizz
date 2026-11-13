use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError};

use buizz_core::api::{
    BacktestGlance, LiveSummary, PresetResearch, Research, RunSummary, Screening, SparkPoint,
    Trials, VariantResult,
};
use buizz_core::market::{RiskFreeRate, Session, Symbol};
use buizz_core::performance::{LivePoint, Metrics};
use buizz_core::run::{CostModel, Mode, Period, PeriodPreset, RunConfig, RunId, RunStatus};
use buizz_core::time::Timestamp;
use buizz_market::MarketStore;
use buizz_metrics::{Screen, Screened};
use buizz_state::{RunFilter, StateStore};
use buizz_supervisor::BotSpec;
use chrono::{NaiveDate, TimeDelta};
use futures::future::try_join;
use serde_json::{Map, Value};
use tokio::sync::OnceCell;

use crate::{Inner, Runner, RunnerError, report};

const SCREENED_STATUSES: [RunStatus; 5] = [
    RunStatus::Queued,
    RunStatus::Starting,
    RunStatus::Running,
    RunStatus::Completed,
    RunStatus::Failed,
];

const FIRST_SESSION_SEARCH_DAYS: i64 = 14;

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
    data_through: Option<Timestamp>,
    figures: Mutex<HashMap<BenchmarkKey, Arc<OnceCell<Figures>>>>,
    judged: Mutex<HashMap<RunId, Arc<Judged>>>,
}

pub struct BotScreening {
    pub screening: Option<Screening>,
    pub backtest: Option<BacktestGlance>,
}

struct Gated {
    judged: Vec<Arc<Judged>>,
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

    async fn screen(&self, run: &RunSummary) -> Result<Arc<Judged>, RunnerError> {
        if !run.status.is_finished() {
            return Ok(Arc::new(Judged {
                screen: buizz_metrics::screen(run, None),
                excess_cagr: None,
                curve: Vec::new(),
            }));
        }
        if let Some(known) = self
            .judged
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&run.id)
        {
            return Ok(Arc::clone(known));
        }
        let judged = Arc::new(self.judge(run).await?);
        self.judged
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(run.id, Arc::clone(&judged));
        Ok(judged)
    }

    async fn judge(&self, run: &RunSummary) -> Result<Judged, RunnerError> {
        let cell = self.figures(run).await?;
        let figures = cell.get().expect("benchmark figures are initialised");
        let equity = self.state.equity(run.id).await?;
        let cagr = |metrics: Option<&Metrics>| metrics.and_then(|metrics| metrics.cagr);
        Ok(Judged {
            screen: buizz_metrics::screen(
                run,
                Some(&Screened {
                    equity: &equity,
                    benchmark: &figures.curve,
                    risk_free: &figures.risk_free,
                }),
            ),
            excess_cagr: cagr(run.metrics.as_ref())
                .zip(cagr(Some(&figures.metrics)))
                .map(|(own, spy)| own - spy),
            curve: buizz_metrics::backtest_curve(&equity, &figures.curve),
        })
    }
}

struct Judged {
    screen: Screen,
    excess_cagr: Option<f64>,
    curve: Vec<LivePoint>,
}

async fn gated(runs: &[RunSummary], benchmarks: &Benchmarks) -> Result<Gated, RunnerError> {
    let mut judged = Vec::with_capacity(runs.len());
    for run in runs {
        judged.push(benchmarks.screen(run).await?);
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
        .zip(&judged)
        .zip(&neighbourhood)
        .filter_map(|((run, judged), median)| {
            judged
                .screen
                .failed_checks
                .is_empty()
                .then_some((run.id, (*median)?))
        })
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(run_id, _)| run_id);
    Ok(Gated {
        judged,
        neighbourhood,
        plateau,
    })
}

fn featured<'a>(
    runs: &'a [RunSummary],
    gated: &'a Gated,
    defaults: &Map<String, Value>,
) -> Option<(&'a RunSummary, Metrics, &'a Arc<Judged>)> {
    let completed = || {
        runs.iter()
            .zip(&gated.judged)
            .filter(|(run, _)| run.status == RunStatus::Completed)
            .filter_map(|(run, judged)| Some((run, run.metrics.clone()?, judged)))
    };
    completed()
        .find(|(run, ..)| Some(run.id) == gated.plateau)
        .or_else(|| completed().find(|(run, ..)| &run.config.params == defaults))
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

fn period_of(run: &RunSummary) -> Period {
    Period {
        from: run.config.start,
        to: run.config.end.unwrap_or(run.config.start),
    }
}

fn gate_preset(presets: &BTreeMap<PeriodPreset, Vec<RunSummary>>) -> Option<PeriodPreset> {
    presets
        .iter()
        .rev()
        .find(|(_, runs)| runs.iter().any(|run| run.status.is_finished()))
        .map(|(preset, _)| *preset)
}

impl Inner {
    pub(crate) fn benchmarks(&self) -> Arc<Benchmarks> {
        let today = Timestamp::now().exchange_date();
        let data_through = self.market.watermarks().borrow().day;
        let mut shared = self
            .benchmarks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match shared.as_ref() {
            Some(current) if current.today == today && current.data_through == data_through => {
                Arc::clone(current)
            }
            _ => Arc::clone(shared.insert(Arc::new(Benchmarks {
                market: self.market.clone(),
                state: self.state.clone(),
                today,
                data_through,
                figures: Mutex::default(),
                judged: Mutex::default(),
            }))),
        }
    }

    pub(crate) fn standard(&self, config: &RunConfig, spec: &BotSpec) -> bool {
        config.cost == CostModel::default()
            && config.risk == spec.manifest.risk
            && config.start_cash == self.config.start_cash
    }

    pub(crate) async fn live_summary(
        &self,
        run: RunSummary,
        started: u32,
    ) -> Result<LiveSummary, RunnerError> {
        let (source, first_session) =
            try_join(self.source_glance(&run), self.first_session(&run)).await?;
        Ok(LiveSummary {
            source,
            first_session,
            ..buizz_metrics::live_summary(run, started)
        })
    }

    async fn source_glance(&self, run: &RunSummary) -> Result<Option<BacktestGlance>, RunnerError> {
        let Some(source) = run.source_run else {
            return Ok(None);
        };
        let Some(source) = self.state.run(source).await? else {
            return Ok(None);
        };
        let preset = match (source.config.end, self.bot(&source.bot).await) {
            (Some(to), Ok(spec)) if self.standard(&source.config, &spec) => PeriodPreset::of(
                Period {
                    from: source.config.start,
                    to,
                },
                source.created_at.exchange_date(),
            ),
            _ => None,
        };
        glance(&source, preset, false, &self.benchmarks()).await
    }

    async fn first_session(&self, run: &RunSummary) -> Result<Option<Session>, RunnerError> {
        if run
            .live_stats
            .as_ref()
            .is_some_and(|stats| stats.sessions > 0)
            || run.status.is_finished()
        {
            return Ok(None);
        }
        let since = run.started_at.unwrap_or(run.created_at);
        let from = since.exchange_date();
        let calendar = self
            .market
            .calendar(from, from + TimeDelta::days(FIRST_SESSION_SEARCH_DAYS))
            .await?;
        Ok(calendar
            .sessions()
            .iter()
            .find(|session| session.close > since)
            .copied())
    }
}

async fn glance(
    run: &RunSummary,
    preset: Option<PeriodPreset>,
    plateau: bool,
    benchmarks: &Benchmarks,
) -> Result<Option<BacktestGlance>, RunnerError> {
    let Some(metrics) = run
        .metrics
        .clone()
        .filter(|_| run.status == RunStatus::Completed)
    else {
        return Ok(None);
    };
    let judged = benchmarks.screen(run).await?;
    Ok(Some(BacktestGlance {
        run_id: run.id,
        preset,
        period: period_of(run),
        plateau,
        failed_checks: judged.screen.failed_checks.clone(),
        sub_periods: judged.screen.sub_periods.clone(),
        params: run.config.params.clone(),
        metrics,
        benchmark: benchmarks.metrics(run).await?,
        benchmark_symbol: run.config.benchmark.clone(),
        curve: judged.curve.clone(),
    }))
}

impl Runner {
    pub fn benchmarks(&self) -> Arc<Benchmarks> {
        self.inner.benchmarks()
    }

    pub async fn live_summary(
        &self,
        run: RunSummary,
        started: u32,
    ) -> Result<LiveSummary, RunnerError> {
        self.inner.live_summary(run, started).await
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
                code: Some(spec.code()),
                mode: Some(Mode::Backtest),
                statuses: SCREENED_STATUSES.to_vec(),
                ..RunFilter::default()
            })
            .await?;
        Ok(latest_variants(runs, |config| {
            self.inner.standard(config, spec)
        }))
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
                backtest: self.latest_backtest(spec, benchmarks).await?,
            });
        };
        let runs = &presets[&preset];
        let gated = gated(runs, benchmarks).await?;
        let finished = || {
            runs.iter()
                .zip(&gated.judged)
                .filter(|(run, _)| run.status.is_finished())
        };
        let screening = Screening {
            preset,
            variants: u32::try_from(finished().count()).unwrap_or(u32::MAX),
            passing: u32::try_from(
                finished()
                    .filter(|(_, judged)| judged.screen.failed_checks.is_empty())
                    .count(),
            )
            .unwrap_or(u32::MAX),
        };
        let backtest = match featured(runs, &gated, &spec.manifest.params) {
            Some((run, metrics, judged)) => Some(BacktestGlance {
                run_id: run.id,
                preset: Some(preset),
                period: period_of(run),
                plateau: gated.plateau == Some(run.id),
                failed_checks: judged.screen.failed_checks.clone(),
                sub_periods: judged.screen.sub_periods.clone(),
                params: run.config.params.clone(),
                metrics,
                benchmark: benchmarks.metrics(run).await?,
                benchmark_symbol: run.config.benchmark.clone(),
                curve: judged.curve.clone(),
            }),
            None => self.latest_backtest(spec, benchmarks).await?,
        };
        Ok(BotScreening {
            screening: Some(screening),
            backtest,
        })
    }

    async fn latest_backtest(
        &self,
        spec: &BotSpec,
        benchmarks: &Benchmarks,
    ) -> Result<Option<BacktestGlance>, RunnerError> {
        let latest = self
            .inner
            .state
            .runs(RunFilter {
                bot: Some(spec.name.clone()),
                code: Some(spec.code()),
                mode: Some(Mode::Backtest),
                statuses: vec![RunStatus::Completed],
                limit: Some(1),
                ..RunFilter::default()
            })
            .await?;
        match latest.into_iter().next() {
            Some(run) => glance(&run, None, false, benchmarks).await,
            None => Ok(None),
        }
    }

    pub async fn research(&self, bot: &str) -> Result<Research, RunnerError> {
        let spec = self.inner.bot(bot).await?;
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
        judged,
        neighbourhood,
        plateau,
    } = gated(&runs, benchmarks).await?;
    let variants = runs
        .into_iter()
        .zip(judged)
        .zip(neighbourhood)
        .map(|((run, judged), neighbourhood_sharpe)| VariantResult {
            run_id: run.id,
            status: run.status,
            period: period_of(&run),
            params: run.config.params,
            metrics: run.metrics,
            badges: run.badges,
            failed_checks: judged.screen.failed_checks.clone(),
            sub_periods: judged.screen.sub_periods.clone(),
            excess_cagr: judged.excess_cagr,
            neighbourhood_sharpe,
            curve: judged.curve.clone(),
        })
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
