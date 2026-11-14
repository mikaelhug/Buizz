use buizz_core::api::{RunSummary, SpineEvent};
use buizz_core::run::{RunId, RunStatus};
use buizz_core::time::Timestamp;
use chrono::TimeDelta;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde_json::json;

use crate::fake::FakeProviders;
use crate::live::{alive, assert_matches_backtest, bot_pids, through_the_close};
use crate::spine::{Options, Spine, eventually, now};
use crate::world::World;

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
    let fake = FakeProviders::serve(World::live(now())).await;
    let mut spine = Spine::start(
        fake.clone(),
        Options {
            bots: &["tape"],
            tiingo: false,
            screening: false,
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

    let deciding = *bot_started(&spine, run.id, 3).await.last().expect("a bot");
    killpg(Pid::from_raw(deciding), Signal::SIGSTOP).expect("the bot is stopped");
    let mut events = spine.subscribe().await;
    let (_, through) = fake.release(1);
    let visible = through + TimeDelta::minutes(1);
    loop {
        if let SpineEvent::Equity { run_id, at, .. } = events.next().await
            && run_id == run.id
            && at == visible
        {
            break;
        }
    }
    killpg(Pid::from_raw(deciding), Signal::SIGKILL).expect("the stopped bot is killed");
    eventually("the undelivered step to be saved", || async {
        (spine.run(run.id).await.clock == Some(visible)).then_some(())
    })
    .await;
    spine.kill().await;
    spine.restart().await;
    let seen = format!("AAA@{}", through.to_datetime().format("%Y-%m-%dT%H:%M:%SZ"));
    eventually(
        "the saved step to reach the bot after the restart",
        || async {
            let logs = spine.logs(run.id).await;
            logs.iter()
                .any(|(_, message)| message.starts_with("step ") && message.contains(&seen))
                .then_some(())
        },
    )
    .await;

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

#[tokio::test(flavor = "multi_thread")]
async fn a_crashed_live_bot_whose_code_changed_is_not_started_again() {
    let fake = FakeProviders::serve(World::live(now())).await;
    let spine = Spine::start(
        fake.clone(),
        Options {
            bots: &[],
            tiingo: false,
            screening: false,
        },
    )
    .await;
    let bot = spine.copy_bot("tape");
    spine.ingested().await;
    let run: RunSummary = spine.post("/api/bots/tape/live", json!({})).await;
    let first = bot_started(&spine, run.id, 1).await[0];

    let script = bot.join("bot.py");
    let edited = std::fs::read_to_string(&script).expect("the bot script") + "\n";
    std::fs::write(&script, edited).expect("the edited bot script");
    killpg(Pid::from_raw(first), Signal::SIGKILL).expect("the bot's process group is killed");
    fake.release(RELEASE_MINUTES);

    let paused = eventually("the run to pause", || async {
        let run = spine.run(run.id).await;
        (run.status == RunStatus::Paused).then_some(run)
    })
    .await;
    assert!(
        paused
            .message
            .as_deref()
            .is_some_and(|message| message.contains("changed since this run started")),
        "{:?}",
        paused.message
    );
    assert_eq!(
        bot_pids(&spine.events(run.id).await),
        [first],
        "no bot process starts under the edited code"
    );
}
