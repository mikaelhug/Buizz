use std::collections::BTreeMap;

use buizz_core::api::{PositionView, RunSummary};
use buizz_core::order::OrderStatus;
use buizz_core::run::{RunBadge, RunEventKind};
use buizz_core::time::Timestamp;
use buizz_sources::wikipedia::{SP500_CHANGES, SP500_CONSTITUENTS, SP500_MEMBERS};
use serde_json::{Value, json};

use crate::fake::FakeProviders;
use crate::spine::{Options, Spine};
use crate::world::{
    Company, Filing, World, decimal, recorded_earnings_insight, stamp, submissions,
};

const RESULTS: &str = "0009999001-24-000050";

fn constituents() -> String {
    let unpriced: String = (2..*SP500_MEMBERS.start())
        .map(|n| format!("\n|-\n|| {{{{NyseSymbol|Z{n:03}}}}}\n|| [[Unpriced Fixture {n}]]"))
        .collect();
    format!(
        r#"Intro.
{{| class="wikitable sortable" id="constituents"
|-
! Symbol !! Security
|-
|| {{{{NyseSymbol|AAA}}}}
|| [[Alpha Fixture Corp]]
|-
|| {{{{NasdaqSymbol|BBB}}}}
|| [[Beta Fixture Inc]]{unpriced}
|}}"#
    )
}

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
                report_date: "2024-11-26",
                accepted: stamp("2024-11-26T21:30:00Z"),
            },
            Filing {
                accession: "0009999001-24-000051",
                form: "8-K/A",
                items: "2.02,9.01",
                report_date: "2024-11-26",
                accepted: stamp("2024-11-29T14:00:00Z"),
            },
            Filing {
                accession: "0009999001-24-000049",
                form: "8-K",
                items: "8.01",
                report_date: "2024-11-22",
                accepted: stamp("2024-11-22T13:00:00Z"),
            },
        ],
    );
    let folder = format!("{}/{}", beta.cik, RESULTS.replace('-', ""));
    world.archives.extend([
        (
            format!("{folder}/{RESULTS}-index.htm"),
            r#"<table class="tableFile" summary="Document Format Files">
<tr><th>Seq</th><th>Description</th><th>Document</th><th>Type</th><th>Size</th></tr>
<tr><td>1</td><td>8-K</td><td><a href="bbb-8k.htm">bbb-8k.htm</a></td><td>8-K</td><td>900</td></tr>
<tr><td>2</td><td>PRESS RELEASE</td><td><a href="bbb-ex991.htm">bbb-ex991.htm</a></td><td>EX-99.1</td><td>700</td></tr>
</table>"#
                .to_owned(),
        ),
        (
            format!("{folder}/bbb-8k.htm"),
            "<html><body><p>Item 2.02 Results of Operations and Financial Condition</p>\
             <p>See the press release furnished as Exhibit 99.1.</p></body></html>"
                .to_owned(),
        ),
        (
            format!("{folder}/bbb-ex991.htm"),
            "<html><body><p>Beta Fixture reports third quarter results</p>\
             <table><tr><td>Revenue</td><td>$</td><td>1,250</td></tr></table></body></html>"
                .to_owned(),
        ),
    ]);
    world.companies.push(Company {
        cik: 9_999_002,
        ticker: "AAA".into(),
        facts: json!({"cik": 9_999_002, "entityName": "AAA", "facts": {}}),
        submissions: submissions(9_999_002, "6022", &[]),
        older_filings: Vec::new(),
    });
    world.earnings_insight = Some(recorded_earnings_insight());
    world.wikipedia = BTreeMap::from([
        (SP500_CONSTITUENTS.to_owned(), constituents()),
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
async fn bots_see_index_membership_profiles_earnings_and_8k_events_as_of_their_clock() {
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
    let health: Value = spine.get("/api/data/health").await;
    let documents = health["backfill"]
        .as_array()
        .expect("backfill progress")
        .iter()
        .find(|item| item["dataset"] == "filing_documents")
        .expect("8-K documents report their backfill");
    assert_eq!(
        documents["done"], documents["total"],
        "every 8-K is read once, an unreadable one recorded as such: {documents}"
    );
    let stored: Vec<std::path::PathBuf> = spine
        .text_dirs()
        .iter()
        .inspect(|dir| assert!(dir.join("volume.json").is_file(), "{dir:?} names itself"))
        .flat_map(|dir| parquet_files(dir))
        .collect();
    assert_eq!(
        stored.len(),
        1,
        "the readable 8-K's documents are one Parquet file on one volume: {stored:?}"
    );

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
    assert_eq!(
        at("2024-11-22T21:00:00Z")["forward_pe"],
        json!([]),
        "the Earnings Insight published on Friday is not known at its close"
    );
    assert_eq!(
        at("2024-11-25T21:00:00Z")["forward_pe"],
        json!([["2024-11-22", "22", "2024-11-22", "2024-11-25T14:30:00Z"]]),
        "its forward P/E is known from Monday's open"
    );
    assert_eq!(at("2024-11-26T21:00:00Z")["earnings"], json!([]));
    assert_eq!(
        at("2024-11-27T21:00:00Z")["earnings"],
        json!([RESULTS]),
        "a release accepted after the close is seen on the next step"
    );
    assert_eq!(
        at("2024-12-02T21:00:00Z")["earnings"],
        json!([RESULTS]),
        "an 8-K/A amending the release is not a second earnings event"
    );
    let other_event = json!(["0009999001-24-000049", ["8.01"]]);
    assert_eq!(at("2024-11-22T21:00:00Z")["filings"], json!([other_event]));
    assert_eq!(
        at("2024-11-26T21:00:00Z")["filings"],
        json!([other_event]),
        "an 8-K accepted after the close is not seen at that close"
    );
    assert_eq!(at("2024-11-26T21:00:00Z")["press"], json!([]));
    assert_eq!(
        at("2024-11-27T21:00:00Z")["filings"],
        json!([other_event, [RESULTS, ["2.02", "9.01"]]])
    );
    assert_eq!(
        at("2024-11-27T21:00:00Z")["press"],
        json!([[
            RESULTS,
            "Beta Fixture reports third quarter results\n\nRevenue\t$\t1,250"
        ]]),
        "the press release exhibit is plain text, its table one tab-separated line per row"
    );
    assert_eq!(
        at("2024-12-02T21:00:00Z")["filings"],
        json!([
            other_event,
            [RESULTS, ["2.02", "9.01"]],
            ["0009999001-24-000051", ["2.02", "9.01"]]
        ]),
        "every 8-K/A is a filing of its own"
    );
    assert_eq!(
        at("2024-12-02T21:00:00Z")["press"],
        at("2024-11-27T21:00:00Z")["press"],
        "an amendment whose filing index cannot be read has no press release"
    );
    for badge in [
        RunBadge::ThinIndexHistory,
        RunBadge::SurvivorshipBiased,
        RunBadge::DataWarnings,
    ] {
        assert!(run.badges.contains(&badge), "{badge:?} in {:?}", run.badges);
    }
    let hedge = spine.orders(run.id).await;
    let [spy] = hedge.as_slice() else {
        panic!("one SPY order: {hedge:#?}")
    };
    assert_eq!(spy.order.symbol.as_str(), "SPY");
    assert_eq!(spy.order.status, OrderStatus::Filled, "{spy:#?}");
    assert_eq!(spy.order.updated_at, stamp("2024-11-21T14:30:00Z"));
    let positions: Vec<PositionView> = spine.get(&format!("/api/runs/{}/positions", run.id)).await;
    assert_eq!(
        positions
            .iter()
            .map(|position| (position.symbol.as_str(), position.last_price))
            .collect::<Vec<_>>(),
        [("SPY", decimal("607.70"))],
        "an instrument outside the universe is marked once it is held"
    );
    let warnings: Vec<String> = spine
        .events(run.id)
        .await
        .into_iter()
        .filter_map(|event| match event.kind {
            RunEventKind::DataWarning { message } => Some(message),
            _ => None,
        })
        .collect();
    let (unpublished, warnings): (Vec<String>, Vec<String>) = warnings
        .into_iter()
        .partition(|warning| warning.contains("macro series"));
    assert_eq!(unpublished.len(), 1, "{unpublished:?}");
    assert!(
        unpublished[0].contains("sp500_forward_pe"),
        "{unpublished:?}"
    );
    let (removed, unpriced) = warnings.split_first().expect("data warnings");
    assert_eq!(removed, "CCC has no 1d bars from 2024-11-20 to 2024-11-22");
    assert_eq!(unpriced.len(), SP500_MEMBERS.start() - 2);
    assert!(
        unpriced.iter().all(|warning| warning.starts_with('Z')
            && warning.ends_with("has no 1d bars from 2024-11-20 to 2024-12-04")),
        "{unpriced:?}"
    );

    let (_, stocks) = backtest(&spine, "reference_stocks").await;
    assert_eq!(
        stocks[&stamp("2024-12-04T21:00:00Z")]["universe"],
        json!(["AAA", "BBB"]),
        "SPY trades the most dollars but is a fund"
    );
    assert_eq!(
        stocks[&stamp("2024-11-26T21:00:00Z")]["bars"],
        json!(["AAA", "BBB", "SPY"])
    );
    assert_eq!(
        stocks[&stamp("2024-12-04T21:00:00Z")]["bars"],
        json!(["AAA", "BBB"]),
        "a sold instrument outside the universe stops reaching the bot"
    );
}

fn parquet_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                parquet_files(&path)
            } else if path
                .extension()
                .is_some_and(|extension| extension == "parquet")
            {
                vec![path]
            } else {
                Vec::new()
            }
        })
        .collect()
}
