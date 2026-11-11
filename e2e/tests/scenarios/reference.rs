use std::collections::BTreeMap;

use buizz_core::api::RunSummary;
use buizz_core::run::{RunBadge, RunEventKind};
use buizz_core::time::Timestamp;
use buizz_sources::wikipedia::{SP500_CHANGES, SP500_CONSTITUENTS};
use serde_json::{Value, json};

use crate::fake::FakeProviders;
use crate::spine::{Options, Spine};
use crate::world::{Company, Filing, World, stamp, submissions};

const RESULTS: &str = "0009999001-24-000050";

const CONSTITUENTS: &str = r#"Intro.
{| class="wikitable sortable" id="constituents"
|-
! Symbol !! Security
|-
|| {{NyseSymbol|AAA}}
|| [[Alpha Fixture Corp]]
|-
|| {{NasdaqSymbol|BBB}}
|| [[Beta Fixture Inc]]
|}"#;

const CHANGES: &str = r#"Intro.
{| class="wikitable sortable" id="changes"
|-
! Effective Date !! Added !! Security !! Removed !! Security !! Reason
|-
|| November 25, 2024
|| BBB
|| [[Beta Fixture Inc]]
|| CCC
|| [[Gamma Fixture Co]]
|| Market capitalization changes.
|}"#;

fn world(mut world: World) -> World {
    let beta = world
        .companies
        .iter_mut()
        .find(|company| company.ticker.as_str() == "BBB")
        .expect("the fixture files for BBB");
    beta.submissions = submissions(
        beta.cik,
        "3571",
        &[
            Filing {
                accession: RESULTS,
                form: "8-K",
                items: "2.02,9.01",
                accepted: stamp("2024-11-26T21:30:00Z"),
            },
            Filing {
                accession: "0009999001-24-000049",
                form: "8-K",
                items: "8.01",
                accepted: stamp("2024-11-22T13:00:00Z"),
            },
        ],
    );
    world.companies.push(Company {
        cik: 9_999_002,
        ticker: "AAA".into(),
        facts: json!({"cik": 9_999_002, "entityName": "AAA", "facts": {}}),
        submissions: submissions(9_999_002, "6022", &[]),
        older_filings: Vec::new(),
    });
    world.wikipedia = BTreeMap::from([
        (SP500_CONSTITUENTS.to_owned(), CONSTITUENTS.to_owned()),
        (SP500_CHANGES.to_owned(), CHANGES.to_owned()),
    ]);
    world
}

async fn backtest(spine: &Spine, bot: &str) -> (RunSummary, BTreeMap<Timestamp, Value>) {
    let queued: RunSummary = spine
        .post(
            &format!("/api/bots/{bot}/backtests"),
            json!({"period": {"from": "2024-11-20", "to": "2024-12-04"}}),
        )
        .await;
    let run = spine.finished(queued.id).await;
    let reports = spine
        .logs(run.id)
        .await
        .into_iter()
        .filter_map(|(_, message)| serde_json::from_str::<Value>(&message).ok())
        .map(|report| (stamp(report["clock"].as_str().expect("a clock")), report))
        .collect();
    (run, reports)
}

#[tokio::test(flavor = "multi_thread")]
async fn bots_see_index_membership_profiles_and_earnings_as_of_their_clock() {
    let world = world(World::fixture_market().await);
    let spine = Spine::start(
        FakeProviders::serve(world).await,
        Options {
            bots: &["reference", "reference_stocks"],
            tiingo: false,
        },
    )
    .await;
    spine.ingested().await;

    let (run, reports) = backtest(&spine, "reference").await;
    let at = |clock: &str| &reports[&stamp(clock)];
    assert_eq!(at("2024-11-22T21:00:00Z")["universe"], json!(["AAA"]));
    assert_eq!(
        at("2024-11-25T21:00:00Z")["universe"],
        json!(["AAA", "BBB"]),
        "BBB joins on its effective date; CCC never had prices"
    );
    assert_eq!(
        at("2024-11-25T21:00:00Z")["instruments"],
        json!({
            "AAA": ["NYSE", "stock", 6022],
            "BBB": ["NASDAQ", "stock", 3571],
            "SPY": ["ARCA", "etf", null],
        })
    );
    assert_eq!(at("2024-11-26T21:00:00Z")["earnings"], json!([]));
    assert_eq!(
        at("2024-11-27T21:00:00Z")["earnings"],
        json!([RESULTS]),
        "a release accepted after the close is seen on the next step"
    );
    for badge in [
        RunBadge::ThinIndexHistory,
        RunBadge::SurvivorshipBiased,
        RunBadge::DataWarnings,
    ] {
        assert!(run.badges.contains(&badge), "{badge:?} in {:?}", run.badges);
    }
    let warnings: Vec<String> = spine
        .events(run.id)
        .await
        .into_iter()
        .filter_map(|event| match event.kind {
            RunEventKind::DataWarning { message } => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(
        warnings,
        ["CCC has no 1d bars from 2024-11-20 to 2024-11-24"]
    );

    let (_, stocks) = backtest(&spine, "reference_stocks").await;
    assert_eq!(
        stocks[&stamp("2024-12-04T21:00:00Z")]["universe"],
        json!(["AAA", "BBB"]),
        "SPY trades the most dollars but is a fund"
    );
}
