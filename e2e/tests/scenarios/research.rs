use buizz_core::api::{ApiError, Research, RunSummary, SweepStarted};
use buizz_core::run::{DesiredState, RunStatus};
use reqwest::{Method, StatusCode};
use serde_json::json;
use tokio::process::Command;

use crate::browser;
use crate::fake::FakeProviders;
use crate::spine::{Options, Spine, USERNAME, eventually, now};
use crate::world::{World, repository};

async fn cli_sweep(spine: &Spine) -> Vec<String> {
    let output = Command::new("uv")
        .args(["run", "--frozen", "--quiet", "--project"])
        .arg(repository().join("sdk/python"))
        .args([
            "buizz",
            "sweep",
            "bots/sma_cross",
            "--preset",
            "1y",
            "--grid",
            "fast=5,10",
            "--grid",
            "slow=20,40.0",
        ])
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
async fn a_swept_grid_runs_once_and_two_of_its_variants_go_live_side_by_side() {
    let fake = FakeProviders::serve(World::screening(now())).await;
    let spine = Spine::start(
        fake,
        Options {
            bots: &["sma_cross"],
            tiingo: false,
        },
    )
    .await;
    spine.ingested().await;

    let printed = cli_sweep(&spine).await;
    assert!(
        printed[0].starts_with("4 runs queued for sma_cross 1y")
            && printed[0].ends_with(", 0 already run"),
        "{printed:?}"
    );
    assert_eq!(printed[1], "trials: 4 variants, 4 runs");
    let again: SweepStarted = spine
        .post(
            "/api/bots/sma_cross/sweeps",
            json!({"preset": "1y", "grid": {"fast": [5.0, 10], "slow": [20, 40]}}),
        )
        .await;
    assert!(again.queued.is_empty(), "{again:?}");
    assert_eq!(again.skipped, 4);
    assert_eq!((again.trials.variants, again.trials.runs), (4, 4));

    let research = eventually("the sweep to finish", || async {
        let research: Research = spine.get("/api/bots/sma_cross/research").await;
        research
            .presets
            .first()
            .is_some_and(|preset| {
                preset.variants.len() == 4
                    && preset
                        .variants
                        .iter()
                        .all(|variant| variant.status == RunStatus::Completed)
            })
            .then_some(research)
    })
    .await;
    let variants = &research.presets[0].variants;
    let mut grid: Vec<(i64, i64)> = variants
        .iter()
        .map(|variant| {
            (
                variant.params["fast"].as_i64().expect("a whole fast"),
                variant.params["slow"].as_i64().expect("a whole slow"),
            )
        })
        .collect();
    grid.sort_unstable();
    assert_eq!(grid, [(5, 20), (5, 40), (10, 20), (10, 40)]);

    let seen = browser::drive("research", &spine, &[]).await;
    let shown: Vec<&str> = seen["params"]
        .as_array()
        .expect("the live rows' params")
        .iter()
        .filter_map(|params| params.as_str())
        .collect();
    assert_eq!(shown.len(), 2, "two live rows: {seen}");
    assert_ne!(shown[0], shown[1], "each row names its variant");

    let live: Vec<RunSummary> = spine.get("/api/runs?bot=sma_cross&mode=live").await;
    assert_eq!(live.len(), 2);
    for run in &live {
        let source = run.source_run.expect("a live variant records its backtest");
        let backtest = spine.run(source).await;
        assert_eq!(run.config.params, backtest.config.params);
        assert_eq!(run.config.risk, backtest.config.risk);
    }
    assert_eq!(
        live.iter()
            .filter(|run| run.desired == DesiredState::Paused)
            .count(),
        1,
        "pausing one variant leaves the other live"
    );

    let third = variants
        .iter()
        .find(|variant| live.iter().all(|run| run.config.params != variant.params))
        .expect("a variant not yet live");
    let third: RunSummary = spine
        .post(
            "/api/bots/sma_cross/live",
            json!({"from_run": third.run_id}),
        )
        .await;
    let (status, body) = spine
        .request(Method::POST, "/api/bots/sma_cross/live", Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let refusal: ApiError = serde_json::from_str(&body).expect("an API error");
    assert_eq!(
        refusal.error,
        "bot sma_cross already has 3 live variants; stop one first"
    );

    let retired: RunSummary = spine
        .post(
            &format!("/api/runs/{}/cancel", third.id),
            json!({"reason": "made room for the defaults"}),
        )
        .await;
    let retired = spine.ended(retired.id).await;
    assert_eq!(retired.status, RunStatus::Cancelled);
    assert_eq!(
        retired.message.as_deref(),
        Some("made room for the defaults")
    );
    let defaults: RunSummary = spine.post("/api/bots/sma_cross/live", json!({})).await;
    assert_eq!(defaults.source_run, None);
}
