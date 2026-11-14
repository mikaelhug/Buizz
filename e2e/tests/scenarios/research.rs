use buizz_core::api::{ApiError, BotDetail, Research, RunSummary, SweepStarted};
use buizz_core::run::{DesiredState, Period, PeriodPreset, RunOrigin, RunStatus};
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
    let mut spine = Spine::start(
        fake,
        Options {
            bots: &["sma_cross"],
            tiingo: false,
            screening: false,
        },
    )
    .await;
    spine.ingested().await;

    let _own_cash: RunSummary = spine
        .post(
            "/api/bots/sma_cross/backtests",
            json!({"preset": "1y", "params": {"fast": 5, "slow": 20}, "start_cash": "50000"}),
        )
        .await;
    let printed = cli_sweep(&spine).await;
    assert!(
        printed[0].starts_with("4 runs queued for sma_cross 1y")
            && printed[0].ends_with(", 0 already run"),
        "{printed:?}"
    );
    assert_eq!(printed[1], "trials: 4 variants, 5 runs");
    let again: SweepStarted = spine
        .post(
            "/api/bots/sma_cross/sweeps",
            json!({"preset": "1y", "grid": {"fast": [5.0, 10], "slow": [20, 40]}}),
        )
        .await;
    assert!(again.queued.is_empty(), "{again:?}");
    assert_eq!(again.skipped, 4);
    assert_eq!((again.trials.variants, again.trials.runs), (4, 5));

    eventually("the sweep to finish", || async {
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
            .then_some(())
    })
    .await;
    spine.edit_shared_module("unused.py", "WINDOW = 20\n");
    let research: Research = spine.get("/api/bots/sma_cross/research").await;
    assert_eq!(
        (research.gate_preset, research.presets[0].variants.len()),
        (Some(research.presets[0].preset), 4),
        "a shared module sma_cross does not import leaves its screen standing"
    );
    let rerun: RunSummary = spine
        .post(
            "/api/bots/sma_cross/backtests",
            json!({"preset": "1y", "params": {"fast": 5, "slow": 20}}),
        )
        .await;
    spine.kill().await;
    spine.edit_shared_module("unused.py", "WINDOW = 50\n");
    spine.restart().await;
    spine.finished(rerun.id).await;
    let research: Research = spine.get("/api/bots/sma_cross/research").await;
    assert_eq!((research.trials.variants, research.trials.runs), (4, 6));
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

#[tokio::test(flavor = "multi_thread")]
async fn the_spine_screens_its_bots_by_itself_and_a_deploy_moves_a_cut_off_screen_to_the_new_code()
{
    let fake = FakeProviders::serve(World::decade(now())).await;
    let mut spine = Spine::start(
        fake,
        Options {
            bots: &["tape"],
            tiingo: false,
            screening: true,
        },
    )
    .await;
    let bot = spine.copy_bot("overnight");
    spine.ingested().await;
    spine.stop().await;
    spine.restart().await;

    let screen = eventually("the screening backtest to start", || async {
        let runs: Vec<RunSummary> = spine.get("/api/runs?bot=overnight").await;
        runs.into_iter().find(|run| run.clock.is_some())
    })
    .await;
    assert_eq!(screen.origin, RunOrigin::Screening);
    let period = Period {
        from: screen.config.start,
        to: screen.config.end.expect("a backtest ends"),
    };
    assert_eq!(
        PeriodPreset::of(period, screen.created_at.exchange_date()),
        Some(PeriodPreset::TenYears)
    );
    let tape: BotDetail = spine.get("/api/bots/tape").await;
    assert!(tape.runs.is_empty(), "{:?}", tape.runs);
    assert!(
        tape.summary
            .unscreened
            .as_deref()
            .is_some_and(|reason| reason.starts_with("the store lacks the 1m bars")),
        "{:?}",
        tape.summary.unscreened
    );

    spine.kill().await;
    let script = bot.join("bot.py");
    let edited = std::fs::read_to_string(&script).expect("the bot script") + "\n";
    std::fs::write(&script, edited).expect("the edited bot script");
    spine.restart().await;

    let cut_off = spine.ended(screen.id).await;
    assert_eq!(cut_off.status, RunStatus::Failed);
    let runs: Vec<RunSummary> = spine.get("/api/runs?bot=overnight").await;
    let [current] = runs
        .iter()
        .filter(|run| run.id != screen.id)
        .collect::<Vec<_>>()[..]
    else {
        panic!("one backtest on the new code: {runs:?}");
    };
    let message = cut_off.message.as_deref().unwrap_or_default();
    assert!(
        message.starts_with("the code of bot overnight changed since this run started; ")
            && message.contains(&format!("run {}", current.id)),
        "{message}"
    );
    let rescreened = spine.finished(current.id).await;
    assert_eq!(rescreened.origin, RunOrigin::Screening);
    assert_eq!(rescreened.config.params, screen.config.params);
}
