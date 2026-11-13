use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use buizz_core::api::ProcessSlots;
use buizz_core::protocol::{ENV_RUN_ID, ENV_RUN_TOKEN, ENV_SPINE_URL};
use buizz_core::run::{BotManifest, RunId};
use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdout};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};
use tokio::time::Instant;

const EVENT_BUFFER: usize = 1024;
const RESTART_WINDOW: Duration = Duration::from_secs(600);
const MAX_RESTARTS_IN_WINDOW: usize = 5;
const REAP_FALLBACK: Duration = Duration::from_millis(100);
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
    #[error("bot manifest cannot be encoded: {0}")]
    Encode(serde_json::Error),
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
    pub behaviour_hash: String,
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
    let behaviour_hash = behaviour_hash(&dir, &manifest)?;
    Ok(BotSpec {
        name,
        dir,
        manifest,
        content_hash,
        behaviour_hash,
    })
}

const SHARED_PACKAGE: &str = "buizz_bots";
const SHARED_BOT_CODE: [&str; 3] = [SHARED_PACKAGE, "pyproject.toml", "uv.lock"];
const SDK_SOURCE: &str = "sdk/python/src";

pub fn bot_hash(dir: &Path) -> Result<String, SupervisorError> {
    let mut hasher = blake3::Hasher::new();
    hash_tree(dir, Path::new(""), &mut hasher)?;
    if let Some(bots_dir) = dir.parent() {
        for name in SHARED_BOT_CODE {
            let shared = bots_dir.join(name);
            if shared.is_dir() {
                hash_tree(bots_dir, Path::new(name), &mut hasher)?;
            } else if shared.is_file() {
                hash_file(&shared, Path::new(name), &mut hasher)?;
            }
        }
        if let Some(root) = bots_dir.parent()
            && root.join(SDK_SOURCE).is_dir()
        {
            hash_tree(root, Path::new(SDK_SOURCE), &mut hasher)?;
        }
    }
    Ok(hasher.finalize().to_hex().to_string())
}

pub fn behaviour_hash(dir: &Path, manifest: &BotManifest) -> Result<String, SupervisorError> {
    let mut hasher = blake3::Hasher::new();
    let behaviour = BotManifest {
        description: String::new(),
        ..manifest.clone()
    };
    let encoded = serde_json::to_vec(&behaviour).map_err(SupervisorError::Encode)?;
    hash_bytes(BotManifest::FILE_NAME, &encoded, &mut hasher);
    let mut files = Vec::new();
    list_files(dir, Path::new(""), &mut files)?;
    let mut pending = Vec::new();
    for relative in &files {
        let name = relative.to_string_lossy();
        if name == BotManifest::FILE_NAME || name.ends_with(".md") {
            continue;
        }
        let path = dir.join(relative);
        hash_file(&path, relative, &mut hasher)?;
        if name.ends_with(".py") {
            pending.extend(shared_imports(&read_source(&path)?, None));
        }
    }
    let Some(package) = dir.parent().map(|bots_dir| bots_dir.join(SHARED_PACKAGE)) else {
        return Ok(hasher.finalize().to_hex().to_string());
    };
    let mut imported = BTreeMap::new();
    while let Some(module) = pending.pop() {
        if imported.contains_key(&module) {
            continue;
        }
        let Some((relative, own_package)) = module_file(&package, &module) else {
            continue;
        };
        let path = package.join(&relative);
        pending.extend(shared_imports(&read_source(&path)?, Some(&own_package)));
        imported.insert(module, relative);
    }
    let files: BTreeSet<PathBuf> = imported.into_values().collect();
    for relative in files {
        hash_file(
            &package.join(&relative),
            &Path::new(SHARED_PACKAGE).join(&relative),
            &mut hasher,
        )?;
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn read_source(path: &Path) -> Result<String, SupervisorError> {
    fs::read_to_string(path).map_err(|e| SupervisorError::Io(path.to_path_buf(), e))
}

fn module_file(package: &Path, module: &[String]) -> Option<(PathBuf, Vec<String>)> {
    let nested: PathBuf = module.iter().collect();
    let init = nested.join("__init__.py");
    if package.join(&init).is_file() {
        return Some((init, module.to_vec()));
    }
    let (last, parent) = module.split_last()?;
    let file = nested.with_file_name(format!("{last}.py"));
    package
        .join(&file)
        .is_file()
        .then(|| (file, parent.to_vec()))
}

fn logical_statements(source: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut depth = 0_i32;
    for line in source.lines() {
        let code = line.split('#').next().unwrap_or_default();
        let continued = code.trim_end().ends_with('\\');
        current.push(' ');
        current.push_str(code.trim_end().trim_end_matches('\\'));
        depth += code.matches('(').count() as i32 - code.matches(')').count() as i32;
        if depth <= 0 && !continued {
            statements.push(std::mem::take(&mut current));
            depth = 0;
        }
    }
    statements.push(current);
    statements
}

fn shared_imports(source: &str, own_package: Option<&[String]>) -> Vec<Vec<String>> {
    let mut modules = Vec::new();
    let mut found = |base: Vec<String>, names: &[&str]| {
        for depth in 0..=base.len() {
            modules.push(base[..depth].to_vec());
        }
        for name in names {
            let mut module = base.clone();
            module.push((*name).to_owned());
            modules.push(module);
        }
    };
    let shared = |dotted: &str| -> Option<Vec<String>> {
        let mut parts = dotted.split('.');
        (parts.next() == Some(SHARED_PACKAGE)).then(|| parts.map(str::to_owned).collect())
    };
    for statement in logical_statements(source) {
        let statement = statement.trim();
        if let Some(targets) = statement.strip_prefix("import ") {
            for target in targets.split(',') {
                let dotted = target.split_whitespace().next().unwrap_or_default();
                if let Some(base) = shared(dotted) {
                    found(base, &[]);
                }
            }
        } else if let Some(rest) = statement.strip_prefix("from ")
            && let Some((source_module, names)) = rest.split_once(" import ")
        {
            let source_module = source_module.trim();
            let names: Vec<&str> = names
                .split(',')
                .filter_map(|name| {
                    name.trim_matches(|c| c == '(' || c == ')')
                        .split_whitespace()
                        .next()
                })
                .filter(|name| *name != "*")
                .collect();
            let dotted = source_module.trim_start_matches('.');
            let level = source_module.len() - dotted.len();
            let base = match (level, own_package) {
                (0, _) => shared(dotted),
                (_, Some(own)) if level <= own.len() + 1 => {
                    let mut base = own[..own.len() + 1 - level].to_vec();
                    base.extend(
                        dotted
                            .split('.')
                            .filter(|part| !part.is_empty())
                            .map(str::to_owned),
                    );
                    Some(base)
                }
                _ => None,
            };
            if let Some(base) = base {
                found(base, &names);
            }
        }
    }
    modules
}

fn list_files(
    root: &Path,
    relative: &Path,
    files: &mut Vec<PathBuf>,
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
            list_files(root, &child, files)?;
        } else if file_type.is_file() {
            files.push(child);
        }
    }
    Ok(())
}

fn hash_file(
    path: &Path,
    label: &Path,
    hasher: &mut blake3::Hasher,
) -> Result<(), SupervisorError> {
    let bytes = fs::read(path).map_err(|e| SupervisorError::Io(path.to_path_buf(), e))?;
    hash_bytes(&label.to_string_lossy(), &bytes, hasher);
    Ok(())
}

fn hash_bytes(label: &str, bytes: &[u8], hasher: &mut blake3::Hasher) {
    hasher.update(&(label.len() as u64).to_le_bytes());
    hasher.update(label.as_bytes());
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn hash_tree(
    root: &Path,
    relative: &Path,
    hasher: &mut blake3::Hasher,
) -> Result<(), SupervisorError> {
    let mut files = Vec::new();
    list_files(root, relative, &mut files)?;
    for file in files {
        hash_file(&root.join(&file), &file, hasher)?;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupervisorConfig {
    pub spine_url: String,
    pub log_dir: PathBuf,
    pub backtest_slots: usize,
    pub live_slots: usize,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reaped {
    pub code: Option<i32>,
    pub wall: Duration,
    pub peak_memory: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessEvent {
    Started { pid: u32 },
    Line { stream: OutputStream, line: String },
    Exited(Reaped),
}

pub struct BotProcess {
    pid: u32,
    kill_grace: Duration,
    events: mpsc::Receiver<ProcessEvent>,
    exit: watch::Receiver<Option<Reaped>>,
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

    pub async fn wait(&mut self) -> Option<Reaped> {
        match self.exit.wait_for(Option::is_some).await {
            Ok(reaped) => *reaped,
            Err(_) => None,
        }
    }

    pub async fn kill(&mut self) -> Result<Option<Reaped>, SupervisorError> {
        signal_group(self.pid, Signal::SIGTERM)?;
        if let Ok(reaped) = tokio::time::timeout(self.kill_grace, self.wait()).await {
            return Ok(reaped);
        }
        signal_group(self.pid, Signal::SIGKILL)?;
        Ok(self.wait().await)
    }

    pub async fn finish(mut self, grace: Duration) -> Result<Option<Reaped>, SupervisorError> {
        match tokio::time::timeout(grace, self.wait()).await {
            Ok(reaped) => Ok(reaped),
            Err(_) => self.kill().await,
        }
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
                let _ = events.try_send(ProcessEvent::Line { stream, line });
            }
            Ok(None) | Err(_) => match stream {
                OutputStream::Stdout => stdout_open = false,
                OutputStream::Stderr => stderr_open = false,
            },
        }
    }
    let _ = log.flush().await;
}

async fn reap(pid: u32, started: Instant) -> Reaped {
    let mut exits = signal(SignalKind::child()).ok();
    let reaped = loop {
        match wait4::try_wait4_pid(pid) {
            Ok(Some(reaped)) => break Some(reaped),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(pid, %error, "cannot reap a bot process");
                break None;
            }
        }
        match exits.as_mut() {
            Some(exits) => {
                exits.recv().await;
            }
            None => tokio::time::sleep(REAP_FALLBACK).await,
        }
    };
    Reaped {
        code: reaped.and_then(|reaped| reaped.status.code()),
        wall: started.elapsed(),
        peak_memory: reaped.map_or(0, |reaped| reaped.rusage.maxrss),
    }
}

pub struct Restarts {
    delay: Duration,
    recent: VecDeque<Instant>,
}

impl Restarts {
    pub fn after_crash(&mut self) -> Option<(u32, Duration)> {
        let now = Instant::now();
        while self
            .recent
            .front()
            .is_some_and(|at| now.duration_since(*at) > RESTART_WINDOW)
        {
            self.recent.pop_front();
        }
        if self.recent.len() >= MAX_RESTARTS_IN_WINDOW {
            return None;
        }
        self.recent.push_back(now);
        let attempt = self.recent.len() as u32;
        let delay = self.delay.saturating_mul(2u32.saturating_pow(attempt - 1));
        Some((attempt, delay))
    }
}

#[derive(Clone)]
pub struct Supervisor {
    config: Arc<SupervisorConfig>,
    backtest_slots: Arc<Semaphore>,
    live_slots: Arc<Semaphore>,
}

impl Supervisor {
    pub fn new(config: SupervisorConfig) -> Self {
        let config = SupervisorConfig {
            backtest_slots: config.backtest_slots.max(1),
            live_slots: config.live_slots.max(1),
            ..config
        };
        Self {
            backtest_slots: Arc::new(Semaphore::new(config.backtest_slots)),
            live_slots: Arc::new(Semaphore::new(config.live_slots)),
            config: Arc::new(config),
        }
    }

    pub fn slots(&self) -> ProcessSlots {
        let count = |slots: usize| u32::try_from(slots).unwrap_or(u32::MAX);
        ProcessSlots {
            backtest: count(self.config.backtest_slots),
            live: count(self.config.live_slots),
        }
    }

    pub async fn backtest_slot(&self) -> OwnedSemaphorePermit {
        self.backtest_slots
            .clone()
            .acquire_owned()
            .await
            .expect("backtest semaphore is never closed")
    }

    pub async fn live_slot(&self) -> OwnedSemaphorePermit {
        self.live_slots
            .clone()
            .acquire_owned()
            .await
            .expect("live semaphore is never closed")
    }

    pub fn restarts(&self) -> Restarts {
        Restarts {
            delay: self.config.restart_delay,
            recent: VecDeque::new(),
        }
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
        let spawn_failed = |source| SupervisorError::Spawn {
            bot: bot.name.clone(),
            source,
        };
        let started = Instant::now();
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
            .map_err(spawn_failed)?;
        let pid = child.id();
        let piped = (
            child.stdout.take().map(ChildStdout::from_std),
            child.stderr.take().map(ChildStderr::from_std),
        );
        let (Some(Ok(stdout)), Some(Ok(stderr))) = piped else {
            let _ = signal_group(pid, Signal::SIGKILL);
            reap(pid, started).await;
            return Err(spawn_failed(std::io::Error::other(
                "the bot's output pipes cannot be read",
            )));
        };
        let (events_tx, events) = mpsc::channel(EVENT_BUFFER);
        let (exit_tx, exit) = watch::channel(None);
        let _ = events_tx.try_send(ProcessEvent::Started { pid });
        tokio::spawn(async move {
            let pump = tokio::spawn(pump_output(stdout, stderr, log, events_tx.clone()));
            let reaped = reap(pid, started).await;
            let _ = signal_group(pid, Signal::SIGKILL);
            let _ = pump.await;
            let _ = exit_tx.send(Some(reaped));
            let _ = events_tx.send(ProcessEvent::Exited(reaped)).await;
        });
        Ok(BotProcess {
            pid,
            kill_grace: self.config.kill_grace,
            events,
            exit,
        })
    }
}
