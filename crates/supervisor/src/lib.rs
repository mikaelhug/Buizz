use std::collections::VecDeque;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use buizz_core::protocol::{ENV_RUN_ID, ENV_RUN_TOKEN, ENV_SPINE_URL};
use buizz_core::run::{BotManifest, RunId};
use buizz_core::time::Timestamp;
use chrono::Utc;
use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdout, Command};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

const EVENT_BUFFER: usize = 1024;
const RESTART_WINDOW: Duration = Duration::from_secs(600);
const MAX_RESTARTS_IN_WINDOW: usize = 5;
const INHERITED_VARS: [&str; 5] = ["PATH", "HOME", "LANG", "TZ", "TMPDIR"];
const INHERITED_PREFIXES: [&str; 2] = ["UV_", "PYTHON"];
const IGNORED_NAMES: [&str; 6] = [
    "__pycache__",
    ".venv",
    ".git",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
];

#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error("no bot named {0:?}")]
    NoSuchBot(String),
    #[error("bot directory {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("bot {bot}: {source}")]
    Manifest {
        bot: String,
        source: buizz_core::run::ManifestError,
    },
    #[error("cannot spawn bot {bot}: {source}")]
    Spawn { bot: String, source: std::io::Error },
    #[error("cannot signal bot process group {pid}: {source}")]
    Signal { pid: u32, source: nix::Error },
}

#[derive(Clone, Debug, PartialEq)]
pub struct BotSpec {
    pub name: String,
    pub dir: PathBuf,
    pub manifest: BotManifest,
    pub content_hash: String,
}

#[derive(Debug, Default)]
pub struct BotScan {
    pub bots: Vec<BotSpec>,
    pub invalid: Vec<SupervisorError>,
}

pub fn scan_bots(dir: &Path) -> Result<BotScan, SupervisorError> {
    let io = |source| SupervisorError::Io(dir.to_path_buf(), source);
    let mut folders = Vec::new();
    for entry in fs::read_dir(dir).map_err(io)? {
        let entry = entry.map_err(io)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().join(BotManifest::FILE_NAME).is_file() && !name.starts_with('.') {
            folders.push((name, entry.path()));
        }
    }
    folders.sort();
    let mut scan = BotScan::default();
    for (name, path) in folders {
        match load(name, path) {
            Ok(bot) => scan.bots.push(bot),
            Err(error) => scan.invalid.push(error),
        }
    }
    Ok(scan)
}

pub fn load_bot(bots_dir: &Path, name: &str) -> Result<BotSpec, SupervisorError> {
    let mut components = Path::new(name).components();
    let direct_child = matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    );
    let dir = bots_dir.join(name);
    if !direct_child || name.starts_with('.') || !dir.join(BotManifest::FILE_NAME).is_file() {
        return Err(SupervisorError::NoSuchBot(name.to_owned()));
    }
    load(name.to_owned(), dir)
}

fn load(name: String, dir: PathBuf) -> Result<BotSpec, SupervisorError> {
    let manifest_path = dir.join(BotManifest::FILE_NAME);
    let text =
        fs::read_to_string(&manifest_path).map_err(|e| SupervisorError::Io(manifest_path, e))?;
    let manifest = BotManifest::from_toml(&text).map_err(|source| SupervisorError::Manifest {
        bot: name.clone(),
        source,
    })?;
    let content_hash = bot_hash(&dir)?;
    Ok(BotSpec {
        name,
        dir,
        manifest,
        content_hash,
    })
}

pub fn bot_hash(dir: &Path) -> Result<String, SupervisorError> {
    let mut hasher = blake3::Hasher::new();
    hash_tree(dir, Path::new(""), &mut hasher)?;
    Ok(hasher.finalize().to_hex().to_string())
}

fn hash_tree(
    root: &Path,
    relative: &Path,
    hasher: &mut blake3::Hasher,
) -> Result<(), SupervisorError> {
    let current = root.join(relative);
    let io = |source| SupervisorError::Io(current.clone(), source);
    let mut entries = Vec::new();
    for entry in fs::read_dir(&current).map_err(io)? {
        let entry = entry.map_err(io)?;
        let name = entry.file_name();
        let lossy = name.to_string_lossy();
        if IGNORED_NAMES.contains(&lossy.as_ref()) || lossy.ends_with(".pyc") {
            continue;
        }
        entries.push((name, entry.file_type().map_err(io)?));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, file_type) in entries {
        let child = relative.join(&name);
        if file_type.is_dir() {
            hash_tree(root, &child, hasher)?;
        } else if file_type.is_file() {
            let path = root.join(&child);
            let bytes = fs::read(&path).map_err(|e| SupervisorError::Io(path, e))?;
            let label = child.to_string_lossy();
            hasher.update(&(label.len() as u64).to_le_bytes());
            hasher.update(label.as_bytes());
            hasher.update(&(bytes.len() as u64).to_le_bytes());
            hasher.update(&bytes);
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupervisorConfig {
    pub spine_url: String,
    pub log_dir: PathBuf,
    pub max_parallel_backtests: usize,
    pub restart_delay: Duration,
    pub kill_grace: Duration,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Launch {
    pub run_id: RunId,
    pub token: String,
    pub bot: BotSpec,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessEvent {
    Started {
        pid: u32,
    },
    Line {
        stream: OutputStream,
        line: String,
        at: Timestamp,
    },
    Exited {
        code: Option<i32>,
    },
    Restarting {
        attempt: u32,
        delay: Duration,
    },
}

pub struct BotProcess {
    pid: u32,
    kill_grace: Duration,
    events: mpsc::Receiver<ProcessEvent>,
    exit: watch::Receiver<Option<Option<i32>>>,
}

fn inherited(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        INHERITED_VARS.contains(&name)
            || INHERITED_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
    })
}

fn signal_group(pid: u32, signal: Signal) -> Result<(), SupervisorError> {
    match killpg(Pid::from_raw(pid as i32), signal) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(source) => Err(SupervisorError::Signal { pid, source }),
    }
}

impl BotProcess {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn events(&mut self) -> &mut mpsc::Receiver<ProcessEvent> {
        &mut self.events
    }

    pub async fn wait(&mut self) -> Option<i32> {
        match self.exit.wait_for(Option::is_some).await {
            Ok(code) => (*code).flatten(),
            Err(_) => None,
        }
    }

    pub async fn kill(&mut self) -> Result<(), SupervisorError> {
        signal_group(self.pid, Signal::SIGTERM)?;
        let exited = tokio::time::timeout(self.kill_grace, self.wait()).await;
        if exited.is_err() {
            signal_group(self.pid, Signal::SIGKILL)?;
            self.wait().await;
        }
        Ok(())
    }
}

impl Drop for BotProcess {
    fn drop(&mut self) {
        if self.exit.borrow().is_none() {
            let _ = signal_group(self.pid, Signal::SIGKILL);
        }
    }
}

async fn pump_output(
    stdout: ChildStdout,
    stderr: ChildStderr,
    mut log: File,
    events: mpsc::Sender<ProcessEvent>,
) {
    let mut stdout = BufReader::new(stdout).lines();
    let mut stderr = BufReader::new(stderr).lines();
    let (mut stdout_open, mut stderr_open) = (true, true);
    while stdout_open || stderr_open {
        let (stream, line) = tokio::select! {
            line = stdout.next_line(), if stdout_open => (OutputStream::Stdout, line),
            line = stderr.next_line(), if stderr_open => (OutputStream::Stderr, line),
        };
        match line {
            Ok(Some(line)) => {
                let _ = log.write_all(format!("{line}\n").as_bytes()).await;
                let at = Timestamp::from_datetime(Utc::now()).unwrap_or_default();
                let _ = events.try_send(ProcessEvent::Line { stream, line, at });
            }
            Ok(None) | Err(_) => match stream {
                OutputStream::Stdout => stdout_open = false,
                OutputStream::Stderr => stderr_open = false,
            },
        }
    }
    let _ = log.flush().await;
}

pub struct LiveProcess {
    events: mpsc::Receiver<ProcessEvent>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl LiveProcess {
    pub fn events(&mut self) -> &mut mpsc::Receiver<ProcessEvent> {
        &mut self.events
    }

    pub async fn stop(mut self) -> Result<(), SupervisorError> {
        self.events.close();
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        Ok(())
    }
}

impl Drop for LiveProcess {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

#[derive(Clone)]
pub struct Supervisor {
    config: Arc<SupervisorConfig>,
    backtest_slots: Arc<Semaphore>,
}

impl Supervisor {
    pub fn new(config: SupervisorConfig) -> Self {
        let backtest_slots = Arc::new(Semaphore::new(config.max_parallel_backtests.max(1)));
        Self {
            config: Arc::new(config),
            backtest_slots,
        }
    }

    pub async fn backtest_slot(&self) -> OwnedSemaphorePermit {
        self.backtest_slots
            .clone()
            .acquire_owned()
            .await
            .expect("backtest semaphore is never closed")
    }

    pub fn log_path(&self, run_id: RunId) -> PathBuf {
        self.config.log_dir.join(format!("{run_id}.log"))
    }

    pub async fn spawn(&self, launch: &Launch) -> Result<BotProcess, SupervisorError> {
        let bot = &launch.bot;
        let log_dir = &self.config.log_dir;
        tokio::fs::create_dir_all(log_dir)
            .await
            .map_err(|e| SupervisorError::Io(log_dir.clone(), e))?;
        let log_path = self.log_path(launch.run_id);
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .await
            .map_err(|e| SupervisorError::Io(log_path, e))?;
        let (program, args) = bot
            .manifest
            .command
            .split_first()
            .expect("validated manifest has a command");
        let mut child = Command::new(program)
            .args(args)
            .current_dir(&bot.dir)
            .env_clear()
            .envs(std::env::vars_os().filter(|(name, _)| inherited(name)))
            .env(ENV_SPINE_URL, &self.config.spine_url)
            .env(ENV_RUN_ID, launch.run_id.to_string())
            .env(ENV_RUN_TOKEN, &launch.token)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|source| SupervisorError::Spawn {
                bot: bot.name.clone(),
                source,
            })?;
        let pid = child.id().expect("a running child has a pid");
        let (stdout, stderr) = (
            child.stdout.take().expect("stdout is piped"),
            child.stderr.take().expect("stderr is piped"),
        );
        let (events_tx, events) = mpsc::channel(EVENT_BUFFER);
        let (exit_tx, exit) = watch::channel(None);
        let _ = events_tx.try_send(ProcessEvent::Started { pid });
        tokio::spawn(async move {
            let pump = tokio::spawn(pump_output(stdout, stderr, log, events_tx.clone()));
            let code = child.wait().await.ok().and_then(|status| status.code());
            let _ = signal_group(pid, Signal::SIGKILL);
            let _ = pump.await;
            let _ = exit_tx.send(Some(code));
            let _ = events_tx.send(ProcessEvent::Exited { code }).await;
        });
        Ok(BotProcess {
            pid,
            kill_grace: self.config.kill_grace,
            events,
            exit,
        })
    }

    pub fn keep_alive(&self, launch: Launch) -> LiveProcess {
        let (events_tx, events) = mpsc::channel(EVENT_BUFFER);
        let (stop, stopped) = oneshot::channel();
        let supervisor = self.clone();
        let task = tokio::spawn(async move {
            supervisor.run_live(launch, events_tx, stopped).await;
        });
        LiveProcess {
            events,
            stop: Some(stop),
            task: Some(task),
        }
    }

    async fn run_live(
        &self,
        launch: Launch,
        events: mpsc::Sender<ProcessEvent>,
        mut stopped: oneshot::Receiver<()>,
    ) {
        let mut restarts: VecDeque<Instant> = VecDeque::new();
        loop {
            match self.spawn(&launch).await {
                Ok(mut process) => loop {
                    tokio::select! {
                        _ = &mut stopped => {
                            if let Err(error) = process.kill().await {
                                tracing::warn!(bot = %launch.bot.name, %error, "stopping live bot failed");
                            }
                            return;
                        }
                        event = process.events().recv() => match event {
                            Some(event) => {
                                let _ = events.send(event).await;
                            }
                            None => break,
                        },
                    }
                },
                Err(error) => {
                    tracing::warn!(bot = %launch.bot.name, %error, "live bot failed to start");
                }
            }
            let now = Instant::now();
            while restarts
                .front()
                .is_some_and(|at| now.duration_since(*at) > RESTART_WINDOW)
            {
                restarts.pop_front();
            }
            if restarts.len() >= MAX_RESTARTS_IN_WINDOW {
                tracing::error!(bot = %launch.bot.name, "live bot crashed too often, giving up");
                return;
            }
            restarts.push_back(now);
            let attempt = restarts.len() as u32;
            let delay = self
                .config
                .restart_delay
                .saturating_mul(2u32.saturating_pow(attempt - 1));
            let _ = events
                .send(ProcessEvent::Restarting { attempt, delay })
                .await;
            tokio::select! {
                _ = &mut stopped => return,
                _ = tokio::time::sleep(delay) => {}
            }
        }
    }
}
