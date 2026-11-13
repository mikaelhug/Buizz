use std::collections::BTreeMap;

use buizz_core::api::{JobView, PositionView, RunSummary};
use buizz_core::order::OrderStatus;
use buizz_core::run::{RecipeRead, RunBadge, RunEventKind};
use buizz_core::time::Timestamp;
use buizz_sources::wikipedia::{SP500_CHANGES, SP500_CONSTITUENTS, SP500_MEMBERS};
use chrono::TimeDelta;
use serde_json::{Value, json};

use crate::fake::FakeProviders;
use crate::spine::{Options, Spine, eventually};
use crate::world::{
    ClaudeAnswer, Company, Filing, Submitted, World, complete_submission, decimal,
    recorded_earnings_insight, repository, stamp, submissions,
};

const RESULTS: &str = "0009999001-24-000050";
const QUARTERLY: &str = "0009999001-24-000045";
const LISTING_NOTICE: &str = "0009999001-24-000044";
const OUTLOOK: &str = "For the fourth quarter, Beta Fixture expects revenue of $1.30 billion to \
     $1.35 billion.";
const BID_PRICE_NOTICE: &str = "On October 14, 2024, the Company received a letter from the Listing \
     Qualifications Department of The Nasdaq Stock Market LLC stating that it no longer complies \
     with Nasdaq Listing Rule 5550(a)(2), as its bid price closed below $1.00 for 30 consecutive \
     business days.";

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
                accession: QUARTERLY,
                form: "10-Q",
                items: "",
                report_date: "2024-09-28",
                accepted: stamp("2024-10-31T20:30:00Z"),
            },
            Filing {
                accession: LISTING_NOTICE,
                form: "8-K",
                items: "3.01",
                report_date: "2024-11-19",
                accepted: stamp("2024-11-21T21:30:00Z"),
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
    let report = "<html><body><p>Item 2.02 Results of Operations and Financial Condition</p>\
         <p>See the press release furnished as Exhibit 99.1.</p></body></html>";
    let release = format!(
        "<html><body><p>Beta Fixture reports third quarter results</p>\
         <table><tr><td>Revenue</td><td>$</td><td>1,250</td></tr></table>\
         <p>{OUTLOOK}</p></body></html>"
    );
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
        (format!("{folder}/bbb-ex991.htm"), release.clone()),
        (
            format!("{folder}/{RESULTS}.txt"),
            complete_submission(
                RESULTS,
                &[
                    Submitted {
                        kind: "8-K",
                        document: "bbb-8k.htm",
                        description: "8-K",
                        body: report,
                    },
                    Submitted {
                        kind: "EX-99.1",
                        document: "bbb-ex991.htm",
                        description: "PRESS RELEASE",
                        body: &release,
                    },
                ],
            ),
        ),
    ]);
    let guidance = |high: u64| {
        json!({"facts": [{
            "fields": {
                "metric": "revenue",
                "metric_label": "revenue",
                "basis": "not_stated",
                "period_label": "fourth quarter",
                "fiscal_year": 2024,
                "fiscal_quarter": 4,
                "low": 1_300_000_000_u64,
                "high": high,
                "unit": "usd",
                "stated_action": "not_stated",
            },
            "evidence": OUTLOOK,
        }]})
    };
    world.claude.push(ClaudeAnswer {
        reading: OUTLOOK.to_owned(),
        first: guidance(1_400_000_000),
        told: guidance(1_350_000_000),
    });
    let notice = format!("{}/{}", beta.cik, LISTING_NOTICE.replace('-', ""));
    let listing_notice = format!(
        "<html><body><p>Item 3.01 Notice of Delisting or Failure to Satisfy a Continued \
         Listing Rule or Standard; Transfer of Listing.</p><p>{BID_PRICE_NOTICE}</p>\
         </body></html>"
    );
    let quarterly = format!("{}/{}", beta.cik, QUARTERLY.replace('-', ""));
    let quarterly_report =
        std::fs::read_to_string(repository().join("crates/sources/fixtures/edgar/10q_sample.htm"))
            .expect("a recorded 10-Q");
    world.archives.extend([
        (
            format!("{notice}/{LISTING_NOTICE}.txt"),
            complete_submission(
                LISTING_NOTICE,
                &[Submitted {
                    kind: "8-K",
                    document: "bbb-8k-notice.htm",
                    description: "8-K",
                    body: &listing_notice,
                }],
            ),
        ),
        (
            format!("{quarterly}/{QUARTERLY}.txt"),
            complete_submission(
                QUARTERLY,
                &[Submitted {
                    kind: "10-Q",
                    document: "aapl-20260627.htm",
                    description: "10-Q",
                    body: &quarterly_report,
                }],
            ),
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
    let mut spine = Spine::start(
        FakeProviders::serve(world).await,
        Options {
            bots: &["reference", "reference_stocks"],
            tiingo: false,
        },
    )
    .await;
    spine.ingested().await;
    let health: Value = spine.get("/api/data/health").await;
    let documents = health["contracts"]
        .as_array()
        .expect("coverage contracts")
        .iter()
        .find(|item| item["dataset"] == "filing_documents")
        .expect("filing documents measure their contract");
    assert_eq!(
        (&documents["pending"], &documents["state"]),
        (&Value::from(0), &Value::from("complete")),
        "every 8-K, 10-K and 10-Q is read once, an unreadable one recorded as failed: {documents}"
    );
    let quarterly: Value = spine
        .get(&format!("/api/data/text/document?id=edgar:{QUARTERLY}:1"))
        .await;
    let keys: Vec<&str> = quarterly["sections"]
        .as_array()
        .expect("the 10-Q's sections")
        .iter()
        .filter_map(|section| section["key"].as_str())
        .collect();
    assert_eq!(
        keys,
        [
            "10-Q:I-1",
            "10-Q:I-2",
            "10-Q:I-3",
            "10-Q:I-4",
            "10-Q:II-1",
            "10-Q:II-1A",
            "10-Q:II-2",
            "10-Q:II-3",
            "10-Q:II-4",
            "10-Q:II-5",
            "10-Q:II-6",
        ],
        "the 10-Q is split into the items of both its parts, past its table of contents"
    );
    let notice: Value = spine
        .get(&format!(
            "/api/data/text/document?id=edgar:{LISTING_NOTICE}:1"
        ))
        .await;
    let facts: Vec<Value> = notice["facts"]
        .as_array()
        .expect("the 8-K's facts")
        .iter()
        .map(|fact| {
            json!([
                fact["section_key"],
                fact["status"],
                fact["exchange"],
                fact["rule"],
                fact["evidence"]
            ])
        })
        .collect();
    assert_eq!(
        facts,
        [json!([
            "8-K:3.01",
            "ok",
            "nasdaq",
            "5550(a)(2)",
            BID_PRICE_NOTICE
        ])],
        "the Item 3.01 notice's exchange and rule are read by code, pass every check with their \
         quote, and are served from the typed view of the current version"
    );
    let release: Value = spine
        .get(&format!("/api/data/text/document?id=edgar:{RESULTS}:1"))
        .await;
    assert_eq!(
        release["sections"],
        json!([{"key": "8-K:2.02", "start": 0, "end": 105}]),
        "the 8-K's main document is cut at its Item 2.02 heading; the 9.01 it lists has none"
    );
    let stored: Vec<std::path::PathBuf> = spine
        .text_dirs()
        .iter()
        .inspect(|dir| assert!(dir.join("volume.json").is_file(), "{dir:?} names itself"))
        .flat_map(|dir| parquet_files(dir))
        .collect();
    assert_eq!(
        stored.len(),
        2,
        "the readable 8-Ks and the 10-Q are one Parquet file per month filed: {stored:?}"
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
    let listing_event = json!([LISTING_NOTICE, ["3.01"]]);
    assert_eq!(at("2024-11-21T21:00:00Z")["filings"], json!([]));
    assert_eq!(
        at("2024-11-22T21:00:00Z")["filings"],
        json!([listing_event, other_event])
    );
    assert_eq!(
        at("2024-11-26T21:00:00Z")["filings"],
        json!([listing_event, other_event]),
        "an 8-K accepted after the close is not seen at that close"
    );
    assert_eq!(
        at("2024-11-21T21:00:00Z")["listing_notices"],
        json!([]),
        "the notice accepted after Thursday's close is not a fact at that close"
    );
    assert_eq!(
        at("2024-11-22T21:00:00Z")["listing_notices"],
        json!([[
            LISTING_NOTICE,
            "ok",
            "nasdaq",
            "5550(a)(2)",
            "2024-11-22T14:30:00Z"
        ]]),
        "its typed fact is read from Friday's open, when the filing became public"
    );
    assert_eq!(
        run.reproducibility
            .as_ref()
            .map(|reproducibility| reproducibility.recipe_versions.clone()),
        Some(vec![RecipeRead {
            recipe: "listing_notice".to_owned(),
            version: 2
        }]),
        "the run records the recipe version it read, so it replays exactly"
    );
    assert_eq!(at("2024-11-26T21:00:00Z")["press"], json!([]));
    assert_eq!(
        at("2024-11-27T21:00:00Z")["filings"],
        json!([listing_event, other_event, [RESULTS, ["2.02", "9.01"]]])
    );
    assert_eq!(
        at("2024-11-27T21:00:00Z")["press"],
        json!([[
            RESULTS,
            format!("Beta Fixture reports third quarter results\n\nRevenue\t$\t1,250\n{OUTLOOK}")
        ]]),
        "the press release exhibit is plain text, its table one tab-separated line per row"
    );
    assert_eq!(
        at("2024-12-02T21:00:00Z")["filings"],
        json!([
            listing_event,
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
    let (produced, warnings): (Vec<String>, Vec<String>) = warnings
        .into_iter()
        .partition(|warning| warning.contains("listing_notice"));
    assert!(
        matches!(produced.as_slice(), [warning] if warning.starts_with(
            "the bot read listing_notice version 2, whose facts were produced after their filings were public"
        )),
        "a backtest reads facts produced long after the fact, and says so: {produced:?}"
    );
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

    let recipe_job = |jobs: &[JobView]| {
        jobs.iter()
            .find(|job| job.name == "recipe_facts")
            .map(|job| {
                format!(
                    "{:?} {}",
                    job.last_outcome,
                    job.last_message.clone().unwrap_or_default()
                )
            })
            .unwrap_or_else(|| {
                format!("{:?}", jobs.iter().map(|job| &job.name).collect::<Vec<_>>())
            })
    };
    let first = recipe_job(&spine.get::<Vec<JobView>>("/api/data/jobs").await);
    assert!(
        first.contains("earnings_guidance: its gold report passed on")
            && first.contains("1 batches open"),
        "the guidance version earns its fill on the gold test split through Messages, then sends \
         the 2024 release to a batch: {first}"
    );
    spine.stop().await;
    spine.restart_later(TimeDelta::hours(7)).await;
    let guidance: Vec<Value> = eventually("the guidance read from the collected batch", || async {
        let release: Value = spine
            .get(&format!("/api/data/text/document?id=edgar:{RESULTS}:1"))
            .await;
        let facts: Vec<Value> = release["facts"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|fact| {
                json!([
                    fact["section_key"],
                    fact["status"],
                    fact["metric"],
                    fact["fiscal_quarter"],
                    fact["low"],
                    fact["high"],
                    fact["evidence"]
                ])
            })
            .collect();
        (!facts.is_empty()).then_some(facts)
    })
    .await;
    assert_eq!(
        guidance,
        [json!([
            "8-K:2.02",
            "ok",
            "revenue",
            4,
            1_300_000_000.0,
            1_350_000_000.0,
            OUTLOOK
        ])],
        "after a restart the stored batch is collected by its id; the answer whose high was not in \
         its quote is retried once, told what failed, and the corrected fact is served"
    );
    let second = recipe_job(&spine.get::<Vec<JobView>>("/api/data/jobs").await);
    assert!(
        second.contains("committed this month"),
        "the job shows the month's spend against its cap: {second}"
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
