use std::path::Path;

use buizz_core::study::{EventStudy, StudySkip};
use serde_json::json;
use tokio::process::Command;

use crate::expect::{assert_same, scenario};
use crate::fake::FakeProviders;
use crate::spine::{Options, Spine, USERNAME};
use crate::world::{World, date, repository};

const EVENTS: &str = "symbol,at,source\n\
    AAA,2024-11-21T15:00:00Z,form 4\n\
    BBB,2024-11-21T09:00:00-05:00,8-k\n\
    AAA,2024-12-02T14:30:00Z,form 4\n\
    ZZZ,2024-11-22T15:00:00Z,form 4\n\
    BBB,2024-11-25T15:00:00Z,form 4\n";

async fn cli_study(spine: &Spine, events: &Path) -> Vec<String> {
    let output = Command::new("uv")
        .args(["run", "--frozen", "--quiet", "--project"])
        .arg(repository().join("sdk/python"))
        .args(["buizz", "event-study"])
        .arg(events)
        .args(["--benchmark", "SPY"])
        .env("BUIZZ_URL", &spine.base)
        .env("BUIZZ_ADMIN_USERNAME", USERNAME)
        .env("BUIZZ_ADMIN_PASSWORD", &spine.password)
        .output()
        .await
        .expect("the buizz CLI runs");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout.lines().map(str::to_owned).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_event_study_matches_the_hand_computation_and_counts_each_distinct_study_once() {
    let expected = scenario("event_study");
    let mut world = World::fixture_market().await;
    let delisted = date("2024-11-27");
    world
        .daily
        .retain(|bar| bar.symbol.as_str() != "BBB" || bar.start.exchange_date() <= delisted);
    for instrument in &mut world.instruments {
        if instrument.symbol.as_str() == "BBB" {
            instrument.active = false;
        }
    }
    let spine = Spine::start(
        FakeProviders::serve(world).await,
        Options {
            bots: &[],
            tiingo: false,
        },
    )
    .await;
    spine.ingested().await;
    let folder = tempfile::tempdir().expect("a scratch folder");
    let events = folder.path().join("insider_buys.csv");
    std::fs::write(&events, EVENTS).expect("the events file");

    let printed = cli_study(&spine, &events).await;
    assert_eq!(
        printed[..3],
        [
            "study insider_buys: 5 events, 4 measured, 1 skipped, 1 not_traded",
            "data through 2024-12-06T21:00:00Z; benchmarks SPY 4; mean round trip cost 0.12% at 10000 a position",
            "trials 1, luck line t 0.00",
        ],
        "{printed:#?}"
    );
    for line in [
        "       1      4      2        0   +0.04%   -0.26%    50%   0.75    -0.08%   -0.38%     50%   0.62",
        "       5      3      1        1   -0.93%   -0.59%     0%      -    -1.05%   -0.71%      0%      -",
        "       3      3   -1.95%   -2.07%",
    ] {
        assert!(
            printed.iter().any(|printed| printed == line),
            "{line:?} in {printed:#?}"
        );
    }

    let study: EventStudy = spine
        .post("/api/studies", expected["request"].clone())
        .await;
    let study = serde_json::to_value(&study).expect("a study encodes");
    assert_same(&expected["study"], &study, 1e-12, "study");

    let broad: EventStudy = spine
        .post(
            "/api/studies",
            json!({"label": "insider_buys", "events": expected["request"]["events"]}),
        )
        .await;
    let skipped: Vec<_> = broad.events.iter().map(|event| event.skipped).collect();
    assert_eq!(
        skipped,
        [
            Some(StudySkip::BenchmarkNotTraded),
            Some(StudySkip::BenchmarkNotTraded),
            Some(StudySkip::BenchmarkNotTraded),
            Some(StudySkip::NotTraded),
            Some(StudySkip::BenchmarkNotTraded),
        ],
        "the fixture has no S&P 500 members and no IWM bars"
    );
    assert!(
        broad
            .events
            .iter()
            .all(|event| event.benchmark.as_ref().map(|symbol| symbol.as_str()) == Some("IWM"))
    );
    assert_eq!(broad.trials, 2, "a new benchmark is a new trial");
    assert!((broad.luck_t - (2.0 * 2f64.ln()).sqrt()).abs() < 1e-12);
}
