use std::fs;
use std::path::Path;
use std::time::Duration;

use buizz_core::run::{BotManifest, RunId};
use buizz_supervisor::{
    BotSpec, Launch, ProcessEvent, Supervisor, SupervisorConfig, bot_hash, scan_bots,
};
use nix::errno::Errno;
use nix::sys::signal::killpg;
use nix::unistd::Pid;

const MANIFEST: &str = "timeframe = \"1d\"\n[universe]\nsymbols = [\"SPY\"]\n";

fn write_bot(bots: &Path, name: &str, script: &str) -> BotSpec {
    let dir = bots.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("run.sh"), script).unwrap();
    let manifest = format!("command = [\"sh\", \"run.sh\"]\n{MANIFEST}");
    fs::write(dir.join("bot.toml"), &manifest).unwrap();
    BotSpec {
        name: name.into(),
        manifest: BotManifest::from_toml(&manifest).unwrap(),
        content_hash: bot_hash(&dir).unwrap(),
        dir,
    }
}

fn gone(pid: u32) -> bool {
    killpg(Pid::from_raw(pid as i32), None) == Err(Errno::ESRCH)
}

#[tokio::test]
async fn process_groups_logs_and_restarts() {
    let temp = tempfile::tempdir().unwrap();
    let bots = temp.path().join("bots");

    let steady = write_bot(
        &bots,
        "steady",
        "echo out\necho err >&2\nsleep 300 &\necho $! > grandchild.pid\necho ready\nwait\n",
    );
    let crashing = write_bot(&bots, "crashing", "echo boom\nexit 3\n");
    fs::create_dir_all(bots.join("broken")).unwrap();
    fs::write(bots.join("broken/bot.toml"), "command = []\n").unwrap();

    let scan = scan_bots(&bots).unwrap();
    assert_eq!(scan.bots.len(), 2);
    assert_eq!(scan.invalid.len(), 1);
    fs::create_dir_all(steady.dir.join("__pycache__")).unwrap();
    fs::write(steady.dir.join("__pycache__/x.pyc"), "cache").unwrap();
    assert_eq!(bot_hash(&steady.dir).unwrap(), steady.content_hash);

    let supervisor = Supervisor::new(SupervisorConfig {
        spine_url: "ws://127.0.0.1:1".into(),
        log_dir: temp.path().join("logs"),
        max_parallel_backtests: 2,
        restart_delay: Duration::from_millis(10),
        kill_grace: Duration::from_secs(2),
    });

    let mut process = supervisor
        .spawn(&Launch {
            run_id: RunId(1),
            token: "secret".into(),
            bot: steady,
        })
        .await
        .unwrap();
    loop {
        match process.events().recv().await.unwrap() {
            ProcessEvent::Line { line, .. } if line == "ready" => break,
            _ => {}
        }
    }
    let grandchild: u32 = fs::read_to_string(bots.join("steady/grandchild.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(!gone(process.pid()));
    process.kill().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(gone(process.pid()));
    assert!(nix::sys::signal::kill(Pid::from_raw(grandchild as i32), None) == Err(Errno::ESRCH));
    let log = fs::read_to_string(supervisor.log_path(RunId(1))).unwrap();
    assert!(log.contains("out") && log.contains("err") && log.contains("ready"));

    let mut live = supervisor.keep_alive(Launch {
        run_id: RunId(2),
        token: "secret".into(),
        bot: crashing,
    });
    let mut restarts = Vec::new();
    let mut exits = 0;
    while let Some(event) = live.events().recv().await {
        match event {
            ProcessEvent::Restarting { attempt, delay } => restarts.push((attempt, delay)),
            ProcessEvent::Exited { code } => {
                assert_eq!(code, Some(3));
                exits += 1;
            }
            _ => {}
        }
    }
    assert_eq!(exits, 6);
    let expected: Vec<_> = [10, 20, 40, 80, 160]
        .iter()
        .zip(1u32..)
        .map(|(ms, attempt)| (attempt, Duration::from_millis(*ms)))
        .collect();
    assert_eq!(restarts, expected);
    live.stop().await.unwrap();
}

#[tokio::test]
async fn stopping_a_chatty_live_bot_nobody_reads_returns() {
    let temp = tempfile::tempdir().unwrap();
    let bots = temp.path().join("bots");
    let chatty = write_bot(
        &bots,
        "chatty",
        "i=0\nwhile [ $i -lt 3000 ]; do echo line $i; i=$((i+1)); done\nexec sleep 300\n",
    );
    let supervisor = Supervisor::new(SupervisorConfig {
        spine_url: "ws://127.0.0.1:1".into(),
        log_dir: temp.path().join("logs"),
        max_parallel_backtests: 1,
        restart_delay: Duration::from_millis(10),
        kill_grace: Duration::from_secs(2),
    });
    let live = supervisor.keep_alive(Launch {
        run_id: RunId(3),
        token: "secret".into(),
        bot: chatty,
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    tokio::time::timeout(Duration::from_secs(10), live.stop())
        .await
        .expect("stop returns while the event queue is full")
        .unwrap();
}

const SECRET: &str = "BUIZZ_ADMIN_PASSWORD";
const ENVIRONMENT_TEST: &str = "bots_see_only_the_protocol_and_toolchain_environment";

#[tokio::test]
async fn bots_see_only_the_protocol_and_toolchain_environment() {
    if std::env::var_os(SECRET).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", ENVIRONMENT_TEST, "--nocapture"])
            .env(SECRET, "hunter2")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let bot = write_bot(&temp.path().join("bots"), "nosy", "env\n");
    let supervisor = Supervisor::new(SupervisorConfig {
        spine_url: "ws://127.0.0.1:1".into(),
        log_dir: temp.path().join("logs"),
        max_parallel_backtests: 1,
        restart_delay: Duration::from_millis(10),
        kill_grace: Duration::from_secs(2),
    });
    let mut process = supervisor
        .spawn(&Launch {
            run_id: RunId(4),
            token: "token".into(),
            bot,
        })
        .await
        .unwrap();
    let mut names = Vec::new();
    while let Some(event) = process.events().recv().await {
        if let ProcessEvent::Line { line, .. } = event
            && let Some((name, _)) = line.split_once('=')
        {
            names.push(name.to_owned());
        }
    }
    assert!(names.iter().any(|name| name == "BUIZZ_RUN_TOKEN"));
    assert!(names.iter().any(|name| name == "PATH"));
    assert!(!names.iter().any(|name| name == SECRET));
}
