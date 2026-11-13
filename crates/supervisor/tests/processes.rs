use std::fs;
use std::path::Path;
use std::time::Duration;

use buizz_core::run::{BotManifest, RunId};
use buizz_supervisor::{
    BotSpec, Launch, ProcessEvent, Supervisor, SupervisorConfig, SupervisorError, behaviour_hash,
    bot_hash, load_bot, scan_bots,
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
    let manifest = BotManifest::from_toml(&manifest).unwrap();
    BotSpec {
        name: name.into(),
        content_hash: bot_hash(&dir).unwrap(),
        behaviour_hash: behaviour_hash(&dir, &manifest).unwrap(),
        manifest,
        dir,
    }
}

#[test]
fn a_bots_behaviour_hash_covers_its_own_folder_and_the_shared_modules_it_imports() {
    let temp = tempfile::tempdir().unwrap();
    let bots = temp.path().join("bots");
    let shared = bots.join("buizz_bots");
    fs::create_dir_all(bots.join("regime")).unwrap();
    fs::create_dir_all(&shared).unwrap();
    let write = |path: &str, text: &str| fs::write(bots.join(path), text).unwrap();
    write(
        "regime/bot.toml",
        &format!("description = \"breadth\"\ncommand = [\"python\", \"bot.py\"]\n{MANIFEST}"),
    );
    write(
        "regime/bot.py",
        "from buizz_bots.signals import (\n    SmaCache,\n)\n\nfrom buizz import run\n",
    );
    write("buizz_bots/__init__.py", "");
    write(
        "buizz_bots/signals.py",
        "from .sessions import week_ends\n\nWINDOW = 200\n",
    );
    write("buizz_bots/sessions.py", "def week_ends(): ...\n");
    write("buizz_bots/hrp.py", "LINKAGE = \"single\"\n");
    let hashed = || load_bot(&bots, "regime").unwrap();
    let before = hashed();

    write("buizz_bots/hrp.py", "LINKAGE = \"ward\"\n");
    write("regime/BRIEF.md", "Breadth regime notes.\n");
    write(
        "regime/bot.toml",
        &format!(
            "description = \"breadth, reworded\"\ncommand = [\"python\", \"bot.py\"]\n{MANIFEST}"
        ),
    );
    let unrelated = hashed();
    assert_ne!(unrelated.content_hash, before.content_hash);
    assert_eq!(
        unrelated.behaviour_hash, before.behaviour_hash,
        "a module the bot never imports, its notes and its description do not change its behaviour"
    );

    write("buizz_bots/sessions.py", "def week_ends(): return []\n");
    let imported = hashed();
    assert_ne!(
        imported.behaviour_hash, unrelated.behaviour_hash,
        "sessions.py is imported through signals.py"
    );

    write(
        "regime/bot.py",
        "from buizz_bots.signals import (\n    SmaCache,\n)\n\nfrom buizz import run\nWINDOW = 100\n",
    );
    assert_ne!(hashed().behaviour_hash, imported.behaviour_hash);
}

fn gone(pid: u32) -> bool {
    killpg(Pid::from_raw(pid as i32), None) == Err(Errno::ESRCH)
}

#[tokio::test]
async fn process_groups_logs_and_exits() {
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
    fs::create_dir_all(bots.join("buizz_bots")).unwrap();
    fs::write(bots.join("buizz_bots/signals.py"), "WINDOW = 20\n").unwrap();
    assert_ne!(bot_hash(&steady.dir).unwrap(), steady.content_hash);
    fs::remove_dir_all(bots.join("buizz_bots")).unwrap();
    fs::create_dir_all(temp.path().join("sdk/python/src/buizz")).unwrap();
    fs::write(
        temp.path().join("sdk/python/src/buizz/context.py"),
        "LIMIT = 1\n",
    )
    .unwrap();
    assert_ne!(bot_hash(&steady.dir).unwrap(), steady.content_hash);
    fs::remove_dir_all(temp.path().join("sdk")).unwrap();

    let supervisor = Supervisor::new(SupervisorConfig {
        spine_url: "ws://127.0.0.1:1".into(),
        log_dir: temp.path().join("logs"),
        backtest_slots: 2,
        live_slots: 1,
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

    let mut crashing = supervisor
        .spawn(&Launch {
            run_id: RunId(2),
            token: "secret".into(),
            bot: crashing,
        })
        .await
        .unwrap();
    let reaped = crashing.wait().await.unwrap();
    assert_eq!(reaped.code, Some(3));
    assert!(reaped.peak_memory > 0, "{reaped:?}");
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
        backtest_slots: 1,
        live_slots: 1,
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

#[test]
fn bots_load_by_name_only_from_direct_children_of_the_bots_folder() {
    let temp = tempfile::tempdir().unwrap();
    let bots = temp.path().join("bots");
    let inside = write_bot(&bots, "inside", "exit 0\n");
    write_bot(temp.path(), "outside", "exit 0\n");
    assert_eq!(load_bot(&bots, "inside").unwrap(), inside);
    for name in ["../outside", "inside/..", "..", "", "/inside", "missing"] {
        assert!(matches!(
            load_bot(&bots, name),
            Err(SupervisorError::NoSuchBot(_))
        ));
    }
}
