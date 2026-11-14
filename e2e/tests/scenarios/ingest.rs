use std::collections::BTreeMap;
use std::time::Duration;

use buizz_core::api::{DataHealth, JobView};
use buizz_core::market::{DataSource, Dataset, InsiderTrade, Symbol};
use buizz_core::ops::{DataWarning, JobOutcome};
use buizz_market::MarketStore;

use crate::browser;
use crate::fake::FakeProviders;
use crate::spine::{Options, Spine, eventually, now};
use crate::world::{World, date, repository};

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
            screening: false,
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
            ("delistings", Some(JobOutcome::Noop)),
            ("fundamentals", ok),
            ("filings", ok),
            ("insider_trades", ok),
            ("press_releases", ok),
            ("filing_documents", ok),
            ("filing_sections", Some(JobOutcome::Noop)),
            ("recipe_facts", Some(JobOutcome::Noop)),
            ("rates", ok),
            ("borrow", ok),
            ("option_chains", Some(JobOutcome::Noop)),
            ("trading_halts", ok),
            ("etf_holdings", ok),
            ("nasdaq_nordic", ok),
            ("euronext_oslo", ok),
            ("fx_rates", ok),
            ("short_interest", Some(JobOutcome::Noop)),
            ("short_volume", Some(JobOutcome::Noop)),
            ("fails_to_deliver", ok),
            ("page_views", ok),
            ("macro", ok),
            ("news", Some(JobOutcome::Backoff)),
            ("vix", ok),
            ("put_call", ok),
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
    assert_eq!(message(&jobs, "put_call"), "put/call: 81 rows");
    assert_eq!(
        message(&jobs, "fails_to_deliver"),
        "fails to deliver: 4 rows"
    );
    assert_eq!(message(&jobs, "page_views"), "page views: 60 rows");
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
    assert_eq!(
        message(&jobs, "option_chains"),
        "option chains: no session today"
    );
    assert_eq!(
        message(&jobs, "trading_halts"),
        "trading halts: 2 new or changed"
    );
    assert_eq!(
        message(&jobs, "etf_holdings"),
        "ETF holdings: 60 rows",
        "two holdings and two days of shares outstanding for each of the 15 SPDR ETFs"
    );
    assert_eq!(
        message(&jobs, "nasdaq_nordic"),
        "Nasdaq Nordic: 4 rows; 13 listed, 13 new or changed, 1 skipped without a full price",
        "two shares and the 11 benchmark indices; Ericsson's five recorded days less the one without an open, none for the others"
    );
    assert_eq!(
        message(&jobs, "euronext_oslo"),
        "Euronext Oslo: 5 rows; 2 listed, 2 new or changed"
    );
    assert_eq!(message(&jobs, "fx_rates"), "fx rates: 35 rows");
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

async fn stored_insider_trades(world: World) -> Vec<InsiderTrade> {
    let fake = FakeProviders::serve(world).await;
    let mut spine = Spine::start(
        fake,
        Options {
            bots: &[],
            tiingo: false,
            screening: false,
        },
    )
    .await;
    let jobs = spine.ingested().await;
    assert_eq!(
        outcomes(&jobs)["insider_trades"],
        Some(JobOutcome::Ok),
        "{}",
        message(&jobs, "insider_trades")
    );
    spine.stop().await;
    MarketStore::open(&spine.market_path(), 1)
        .await
        .expect("the stopped spine's market store opens")
        .insider_trades(vec![Symbol::new("AAPL")], date("2000-01-01"), now())
        .await
        .expect("insider trades read")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_quarter_of_sec_insider_data_sets_gives_bots_the_trades_its_form_4_documents_give() {
    let mut documents = World::recorded_sources();
    documents.archives.insert(
        "320193/000118143113036353/rrd384222.xml".to_owned(),
        std::fs::read_to_string(
            repository().join("crates/sources/fixtures/edgar/form4_2013_sample.xml"),
        )
        .expect("the recorded 2013 Form 4/A"),
    );
    for files in documents.insider_quarters.values_mut() {
        for text in files.values_mut() {
            text.truncate(text.find('\n').map_or(text.len(), |header| header + 1));
        }
    }
    let read_from_documents = stored_insider_trades(documents).await;
    let read_from_data_sets = stored_insider_trades(World::recorded_sources()).await;

    let schiller = read_from_data_sets
        .iter()
        .find(|trade| trade.accession == "0001181431-13-036353")
        .expect("the 2013 Form 4/A comes from the 2013 Q2 data set, whose XML is never served");
    assert_eq!(schiller.form, "4/A");
    assert_eq!(schiller.accepted_at.to_string(), "2013-06-22T01:45:37Z");
    assert_eq!(
        schiller.owners[0].officer_title.as_deref(),
        Some("Senior Vice President")
    );
    assert_eq!(schiller.transaction.code, "F");
    assert_eq!(read_from_data_sets, read_from_documents);
}
