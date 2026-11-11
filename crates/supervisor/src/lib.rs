use std::path::{Path, PathBuf};
use std::time::Duration;

use buizz_core::run::{BotManifest, RunId};
use buizz_core::time::Timestamp;
use tokio::sync::{OwnedSemaphorePermit, mpsc};

#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
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

pub fn scan_bots(_dir: &Path) -> Result<BotScan, SupervisorError> {
    unimplemented!()
}

pub fn bot_hash(_dir: &Path) -> Result<String, SupervisorError> {
    unimplemented!()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backoff {
    pub initial: Duration,
    pub max: Duration,
}

impl Backoff {
    pub fn delay(&self, _attempt: u32) -> Duration {
        unimplemented!()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupervisorConfig {
    pub spine_url: String,
    pub max_parallel_backtests: usize,
    pub restart: Backoff,
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

pub struct BotProcess {}

impl BotProcess {
    pub fn pid(&self) -> u32 {
        unimplemented!()
    }

    pub fn events(&mut self) -> &mut mpsc::Receiver<ProcessEvent> {
        unimplemented!()
    }

    pub async fn wait(&mut self) -> Option<i32> {
        unimplemented!()
    }

    pub async fn kill(&mut self) -> Result<(), SupervisorError> {
        unimplemented!()
    }
}

pub struct LiveProcess {}

impl LiveProcess {
    pub fn events(&mut self) -> &mut mpsc::Receiver<ProcessEvent> {
        unimplemented!()
    }

    pub async fn stop(self) -> Result<(), SupervisorError> {
        unimplemented!()
    }
}

#[derive(Clone)]
pub struct Supervisor {}

impl Supervisor {
    pub fn new(_config: SupervisorConfig) -> Self {
        unimplemented!()
    }

    pub async fn backtest_slot(&self) -> OwnedSemaphorePermit {
        unimplemented!()
    }

    pub async fn spawn(&self, _launch: &Launch) -> Result<BotProcess, SupervisorError> {
        unimplemented!()
    }

    pub fn keep_alive(&self, _launch: Launch) -> LiveProcess {
        unimplemented!()
    }
}
