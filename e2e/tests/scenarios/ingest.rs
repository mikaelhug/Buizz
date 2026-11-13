use std::collections::BTreeMap;
use std::time::Duration;

use buizz_core::api::{DataHealth, JobView};
use buizz_core::market::{DataSource, Dataset, Symbol};
use buizz_core::ops::{DataWarning, JobOutcome};

use crate::browser;
use crate::fake::FakeProviders;
use crate::spine::{Options, Spine, eventually};
use crate::world::{World, date};

const QUIET_AFTER_RESTART: Duration = Duration::from_secs(2);

fn outcomes(jobs: &[JobView]) -> BTreeMap<&str, Option<JobOutcome>> {
    jobs.iter()
        .map(|job| (job.name.as_str(), job.last_outcome))
        .collect()
}

fn message<'a>(jobs: &'a [JobView], name: &str) -> &'a str {
    jobs.iter()
        .find(|job| job.name == name)
        .and_then(|job| job.last_message.as_deref())
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_spine_backfills_backs_off_rejects_a_bad_tiingo_tail_and_restarts_quietly() {
    let world = World::recorded_sources();
    let last_close = world.session(date("2024-07-05")).close;
    let fake = FakeProviders::serve(world).await;
    fake.rate_limit("corporate_actions", 1);
    let mut spine = Spine::start(
        fake.clone(),
        Options {
            bots: &[],
            tiingo: true,
        },
    )
    .await;

    let jobs = spine.ingested().await;
    let ok = Some(JobOutcome::Ok);
    assert_eq!(
        outcomes(&jobs),
        BTreeMap::from([
            ("calendar", ok),
            ("instruments", ok),
            ("index_membership", ok),
            ("bars_1d", ok),
            ("bars_1m", Some(JobOutcome::Noop)),
            ("corporate_actions", Some(JobOutcome::Backoff)),
            ("sec_index", Some(JobOutcome::Waiting)),
            ("fundamentals", ok),
            ("filings", ok),
            ("insider_trades", ok),
            ("press_releases", ok),
            ("filing_documents", ok),
            ("filing_sections", Some(JobOutcome::Noop)),
            ("rates", ok),
            ("borrow", ok),
            ("short_interest", Some(JobOutcome::Noop)),
            ("short_volume", Some(JobOutcome::Noop)),
            ("macro", ok),
            ("news", Some(JobOutcome::Backoff)),
            ("vix", ok),
            ("fomc", ok),
            ("earnings_insight", ok),
            ("earnings_insight_archive", Some(JobOutcome::Noop)),
            ("state_snapshot", ok),
            ("market_snapshot", ok),
            ("tiingo_history", Some(JobOutcome::Failed)),
        ]),
        "{jobs:#?}"
    );
    assert!(message(&jobs, "corporate_actions").contains("rate limited"));
    assert_eq!(message(&jobs, "instruments"), "instruments: 3 rows");
    assert!(
        message(&jobs, "index_membership").contains("from 9 changes"),
        "{}",
        message(&jobs, "index_membership")
    );
    assert_eq!(
        message(&jobs, "filings"),
        "filings: 14 rows",
        "the company, its 4 earnings releases, its 7 8-Ks and its 2 10-Qs"
    );
    assert_eq!(message(&jobs, "press_releases"), "press releases: 4 rows");
    assert_eq!(message(&jobs, "vix"), "vix: 8 rows");
    assert!(
        message(&jobs, "fomc").starts_with("fomc: 16 rows, 1 failed: 2000"),
        "{}",
        message(&jobs, "fomc")
    );
    assert_eq!(
        message(&jobs, "earnings_insight"),
        "Earnings Insight 2024-11-22: 32 rows"
    );
    assert_eq!(
        message(&jobs, "borrow"),
        "borrow: 6 tickers as of 2026-11-11T20:10:41Z"
    );
    let rejected = message(&jobs, "tiingo_history");
    assert!(
        rejected.contains("AAPL") && rejected.contains("rejected"),
        "{rejected}"
    );

    let health: DataHealth = spine.get("/api/data/health").await;
    assert_eq!(health.watermarks.day, Some(last_close));
    let daily: BTreeMap<String, (String, String)> = health
        .coverage
        .iter()
        .filter(|row| row.dataset == Dataset::Bars1d)
        .map(|row| {
            assert_eq!(row.source, DataSource::Alpaca, "{row:?}");
            (
                row.symbol
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                (row.first.to_string(), row.last.to_string()),
            )
        })
        .collect();
    assert_eq!(
        daily,
        BTreeMap::from([
            (
                "AAPL".to_owned(),
                ("2024-06-03".to_owned(), "2024-07-05".to_owned())
            ),
            (
                "SPY".to_owned(),
                ("2024-06-04".to_owned(), "2024-07-05".to_owned())
            ),
        ])
    );
    let overlap = health
        .warnings
        .iter()
        .find_map(|warning| match warning {
            DataWarning::OverlapRejected {
                symbol,
                source,
                message,
            } => Some((symbol.clone(), *source, message.clone())),
            _ => None,
        })
        .expect("data health shows the rejected Tiingo overlap");
    assert_eq!(overlap.0, Symbol::new("AAPL"));
    assert_eq!(overlap.1, DataSource::Tiingo);
    assert_eq!(
        rejected,
        format!(
            "Tiingo history: all 1 failed: AAPL: Tiingo rejected, {}",
            overlap.2
        )
    );

    browser::drive("data", &spine, &[]).await;

    let mut before = fake.hits();
    before.remove("stream");
    spine.stop().await;
    spine.restart().await;
    eventually(
        "the restarted spine to restore its daily watermark",
        || async {
            let health: DataHealth = spine.get("/api/data/health").await;
            (health.watermarks.day == Some(last_close)).then_some(())
        },
    )
    .await;
    tokio::time::sleep(QUIET_AFTER_RESTART).await;
    let mut after = fake.hits();
    after.remove("stream");
    assert_eq!(after, before, "nothing is due on the next tick");
    let again: Vec<JobView> = spine.get("/api/data/jobs").await;
    assert_eq!(
        again.iter().map(|job| job.last_started).collect::<Vec<_>>(),
        jobs.iter().map(|job| job.last_started).collect::<Vec<_>>()
    );
}
