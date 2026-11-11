use buizz_core::api::RunSummary;
use buizz_core::run::{RunId, RunStatus};
use buizz_core::time::Timestamp;
use chrono::TimeDelta;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde_json::json;

use crate::fake::FakeProviders;
use crate::live::{alive, assert_matches_backtest, bot_pids, live_world, through_the_close};
use crate::spine::{Options, Spine, eventually};

const RELEASE_MINUTES: i64 = 3;

async fn caught_up(spine: &Spine, id: RunId, through: Timestamp) {
    let visible = through + TimeDelta::minutes(1);
    eventually("the live run to catch up", || async {
        let run = spine.run(id).await;
        (run.status == RunStatus::Running && run.clock == Some(visible)).then_some(())
    })
    .await;
}

async fn bot_started(spine: &Spine, id: RunId, count: usize) -> Vec<i32> {
    eventually("the bot process to start", || async {
        let pids = bot_pids(&spine.events(id).await);
        (pids.len() == count).then_some(pids)
    })
    .await
}

fn step_numbers(logs: &[(Option<Timestamp>, String)]) -> Vec<u64> {
    logs.iter()
        .filter_map(|(_, message)| {
            message
                .strip_prefix("step ")
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|step| step.parse().ok())
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_live_run_survives_kill_9_of_its_bot_and_of_the_spine() {
    let fake = FakeProviders::serve(live_world().await).await;
    let mut spine = Spine::start(
        fake.clone(),
        Options {
            bots: &["tape"],
            tiingo: false,
        },
    )
    .await;
    spine.ingested().await;
    let run: RunSummary = spine.post("/api/bots/tape/live", json!({})).await;
    caught_up(&spine, run.id, fake.released()).await;

    let first = bot_started(&spine, run.id, 1).await[0];
    killpg(Pid::from_raw(first), Signal::SIGKILL).expect("the bot's process group is killed");
    let (_, through) = fake.release(RELEASE_MINUTES);
    bot_started(&spine, run.id, 2).await;
    assert!(!alive(first));
    caught_up(&spine, run.id, through).await;

    fake.drop_streams();
    let (_, through) = fake.release(RELEASE_MINUTES);
    caught_up(&spine, run.id, through).await;

    spine.kill().await;
    let (_, through) = fake.release(RELEASE_MINUTES);
    spine.restart().await;
    caught_up(&spine, run.id, through).await;

    fake.release(60);
    let finished = through_the_close(&spine, run.id).await;
    let steps = step_numbers(&spine.logs(run.id).await);
    let expected: Vec<u64> = (1..=u64::try_from(steps.len()).expect("steps fit")).collect();
    assert_eq!(
        steps, expected,
        "the bot resumes from its saved state every time"
    );
    assert_matches_backtest(&spine, &finished).await;
}
