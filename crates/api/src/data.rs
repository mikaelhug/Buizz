use std::collections::BTreeMap;

use axum::Json;
use axum::extract::State;
use buizz_core::api::{
    BackfillProgress, BotProcesses, DataHealth, DataSpine, DatasetSpine, FeedSlots, JobView,
    MarketCoverage, SourceCoverage,
};
use buizz_core::market::{
    Coverage, DataSource, Dataset, HISTORY_START, Market, Symbol, Timeframe, Watermarks,
};
use buizz_core::ops::{CoverageGap, DataWarning};
use buizz_core::time::Timestamp;
use chrono::{Datelike, NaiveDate, TimeDelta};

use crate::AppState;
use crate::error::Failure;

const WARNING_WINDOW_DAYS: i64 = 365;

fn covered_ranges(coverage: &[Coverage]) -> BTreeMap<Symbol, (NaiveDate, NaiveDate)> {
    let mut ranges: BTreeMap<Symbol, (NaiveDate, NaiveDate)> = BTreeMap::new();
    for entry in coverage.iter().filter(|c| c.dataset == Dataset::Bars1d) {
        let Some(symbol) = &entry.symbol else {
            continue;
        };
        ranges
            .entry(symbol.clone())
            .and_modify(|(first, last)| {
                *first = (*first).min(entry.first);
                *last = (*last).max(entry.last);
            })
            .or_insert((entry.first, entry.last));
    }
    ranges
}

fn within_coverage(
    gap: CoverageGap,
    ranges: &BTreeMap<Symbol, (NaiveDate, NaiveDate)>,
) -> Option<DataWarning> {
    let (first, last) = ranges.get(&gap.symbol)?;
    let from = gap.from.max(*first);
    let to = gap.to.min(*last);
    (from <= to).then_some(DataWarning::CoverageGap(CoverageGap { from, to, ..gap }))
}

async fn warnings(
    app: &AppState,
    watermarks: Watermarks,
    coverage: &[Coverage],
) -> Result<Vec<DataWarning>, Failure> {
    let Some(day) = watermarks.day else {
        return Ok(Vec::new());
    };
    let to = day.exchange_date();
    let from = to - TimeDelta::days(WARNING_WINDOW_DAYS);
    let ranges = covered_ranges(coverage);
    let symbols: Vec<Symbol> = ranges.keys().cloned().collect();
    let gaps = app
        .market
        .coverage_gaps(symbols.clone(), Timeframe::Day, from, to)
        .await?;
    let mut warnings: Vec<DataWarning> = gaps
        .into_iter()
        .filter_map(|gap| within_coverage(gap, &ranges))
        .collect();
    warnings.extend(app.market.unexplained_moves(symbols, from, to).await?);
    warnings.extend(
        app.market
            .action_disagreements(None, HISTORY_START, to)
            .await?,
    );
    Ok(warnings)
}

#[utoipa::path(get, path = "/api/data/health", tag = "data",
    responses((status = 200, body = DataHealth)))]
pub(crate) async fn data_health(State(app): State<AppState>) -> Result<Json<DataHealth>, Failure> {
    let watermarks = *app.market.watermarks().borrow();
    let coverage = app.market.coverage().await?;
    let mut warnings = warnings(&app, watermarks, &coverage).await?;
    let rejected = app
        .state
        .latest_rejections(DataSource::Tiingo, Dataset::Bars1d)
        .await?;
    warnings.extend(rejected.into_iter().filter_map(|fetch| {
        Some(DataWarning::OverlapRejected {
            symbol: fetch.symbol?,
            source: fetch.source,
            message: fetch.message.unwrap_or_default(),
        })
    }));
    let stale_jobs = app
        .ingest
        .job_views(Timestamp::now())
        .await?
        .iter()
        .filter(|job| job.stale)
        .count();
    let backfill = app.ingest.backfill_progress().await?;
    Ok(Json(DataHealth {
        watermarks,
        backfill,
        coverage,
        warnings,
        stale_jobs: u32::try_from(stale_jobs).unwrap_or(u32::MAX),
        ingest_enabled: app.ingest.enabled(),
    }))
}

fn month_of(date: NaiveDate) -> i64 {
    i64::from(date.year()) * 12 + i64::from(date.month0())
}

fn monthly(
    ranges: &BTreeMap<Option<&Symbol>, (NaiveDate, NaiveDate)>,
    today: NaiveDate,
) -> Vec<u32> {
    let start = month_of(HISTORY_START);
    let months = usize::try_from(month_of(today) - start + 1).unwrap_or(0);
    let mut steps = vec![0_i64; months + 1];
    let index = |date: NaiveDate| {
        usize::try_from((month_of(date) - start).max(0))
            .unwrap_or(0)
            .min(months.saturating_sub(1))
    };
    for (first, last) in ranges.values() {
        steps[index(*first)] += 1;
        steps[index(*last) + 1] -= 1;
    }
    steps
        .iter()
        .take(months)
        .scan(0_i64, |covered, step| {
            *covered += step;
            Some(u32::try_from(*covered).unwrap_or(0))
        })
        .collect()
}

fn market_coverage(
    market: Market,
    rows: &[&Coverage],
    backfill: Vec<BackfillProgress>,
    today: NaiveDate,
) -> MarketCoverage {
    let mut ranges: BTreeMap<Option<&Symbol>, (NaiveDate, NaiveDate)> = BTreeMap::new();
    let mut sources: BTreeMap<DataSource, SourceCoverage> = BTreeMap::new();
    for row in rows {
        ranges
            .entry(row.symbol.as_ref())
            .and_modify(|(first, last)| {
                *first = (*first).min(row.first);
                *last = (*last).max(row.last);
            })
            .or_insert((row.first, row.last));
        sources
            .entry(row.source)
            .and_modify(|source| {
                source.symbols += u32::from(row.symbol.is_some());
                source.first = source.first.min(row.first);
                source.last = source.last.max(row.last);
                source.updated_at = source.updated_at.max(row.updated_at);
            })
            .or_insert(SourceCoverage {
                source: row.source,
                symbols: u32::from(row.symbol.is_some()),
                first: row.first,
                last: row.last,
                updated_at: row.updated_at,
            });
    }
    MarketCoverage {
        market,
        symbols: u32::try_from(ranges.keys().filter(|symbol| symbol.is_some()).count())
            .unwrap_or(u32::MAX),
        first: ranges.values().map(|(first, _)| *first).min(),
        last: ranges.values().map(|(_, last)| *last).max(),
        updated_at: rows.iter().map(|row| row.updated_at).max(),
        monthly: monthly(&ranges, today),
        sources: sources.into_values().collect(),
        backfill,
    }
}

#[utoipa::path(get, path = "/api/data/spine", tag = "data",
    responses((status = 200, body = DataSpine)))]
pub(crate) async fn data_spine(State(app): State<AppState>) -> Result<Json<DataSpine>, Failure> {
    let watermarks = *app.market.watermarks().borrow();
    let coverage = app.market.coverage().await?;
    let backfill = app.ingest.backfill_progress().await?;
    let today = Timestamp::now().exchange_date();
    let market = Market::Us;
    let datasets = Dataset::ALL
        .into_iter()
        .map(|dataset| {
            let rows: Vec<&Coverage> = coverage
                .iter()
                .filter(|row| row.dataset == dataset)
                .collect();
            let progress = backfill
                .iter()
                .filter(|item| item.dataset == dataset)
                .cloned()
                .collect();
            DatasetSpine {
                dataset,
                domain: dataset.domain(),
                markets: vec![market_coverage(market, &rows, progress, today)],
            }
        })
        .collect();
    Ok(Json(DataSpine {
        markets: vec![market],
        datasets,
        watermarks,
        ingest_enabled: app.ingest.enabled(),
        history_start: HISTORY_START,
        today,
    }))
}

#[utoipa::path(get, path = "/api/data/jobs", tag = "data",
    responses((status = 200, body = Vec<JobView>)))]
pub(crate) async fn data_jobs(State(app): State<AppState>) -> Result<Json<Vec<JobView>>, Failure> {
    Ok(Json(app.ingest.job_views(Timestamp::now()).await?))
}

#[utoipa::path(get, path = "/api/data/feed", tag = "data",
    responses((status = 200, body = FeedSlots)))]
pub(crate) async fn data_feed(State(app): State<AppState>) -> Json<FeedSlots> {
    Json(app.feed.slots().borrow().clone())
}

#[utoipa::path(get, path = "/api/data/processes", tag = "data",
    responses((status = 200, body = BotProcesses)))]
pub(crate) async fn data_processes(
    State(app): State<AppState>,
) -> Result<Json<BotProcesses>, Failure> {
    Ok(Json(BotProcesses {
        slots: app.runner.process_slots(),
        footprints: app.state.footprints().await?,
    }))
}
