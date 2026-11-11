use std::collections::BTreeMap;

use axum::Json;
use axum::extract::State;
use buizz_core::api::{DataHealth, FeedSlots, JobView};
use buizz_core::market::{Coverage, DataSource, Dataset, Symbol, Timeframe, Watermarks};
use buizz_core::ops::{CoverageGap, DataWarning};
use chrono::{NaiveDate, TimeDelta};

use crate::AppState;
use crate::error::Failure;
use crate::time::now;

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
        .job_views(now())
        .await?
        .iter()
        .filter(|job| job.stale)
        .count();
    Ok(Json(DataHealth {
        watermarks,
        coverage,
        warnings,
        stale_jobs: u32::try_from(stale_jobs).unwrap_or(u32::MAX),
        ingest_enabled: app.ingest.enabled(),
    }))
}

#[utoipa::path(get, path = "/api/data/jobs", tag = "data",
    responses((status = 200, body = Vec<JobView>)))]
pub(crate) async fn data_jobs(State(app): State<AppState>) -> Result<Json<Vec<JobView>>, Failure> {
    Ok(Json(app.ingest.job_views(now()).await?))
}

#[utoipa::path(get, path = "/api/data/feed", tag = "data",
    responses((status = 200, body = FeedSlots)))]
pub(crate) async fn data_feed(State(app): State<AppState>) -> Json<FeedSlots> {
    Json(app.feed.slots().borrow().clone())
}
