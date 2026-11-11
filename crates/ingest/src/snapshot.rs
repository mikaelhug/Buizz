use std::path::{Path, PathBuf};

use buizz_core::time::Timestamp;

use crate::fetch::Work;
use crate::{Ingest, IngestError};

const STATE_PREFIX: &str = "state-";
const STATE_SUFFIX: &str = ".db";
const MARKET_PREFIX: &str = "market-";
const PARTIAL_SUFFIX: &str = ".partial";

impl Ingest {
    pub(crate) async fn state_snapshot(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some((target, partial)) = self.prepare(now, STATE_PREFIX, STATE_SUFFIX).await? else {
            return Ok(Work::noop("today's state snapshot already exists"));
        };
        self.state.snapshot(&partial).await?;
        tokio::fs::rename(&partial, &target).await?;
        let pruned = prune(
            &self.config.snapshot_dir,
            STATE_PREFIX,
            STATE_SUFFIX,
            self.config.snapshot_keep,
        )
        .await?;
        Ok(Work::ok(
            1,
            format!("wrote {}, pruned {pruned}", target.display()),
        ))
    }

    pub(crate) async fn market_snapshot(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some((target, partial)) = self.prepare(now, MARKET_PREFIX, "").await? else {
            return Ok(Work::noop("today's market snapshot already exists"));
        };
        tokio::fs::create_dir_all(&partial).await?;
        let rows = self.market.export_unrecoverable(partial.clone()).await?;
        tokio::fs::rename(&partial, &target).await?;
        let pruned = prune(
            &self.config.snapshot_dir,
            MARKET_PREFIX,
            "",
            self.config.snapshot_keep,
        )
        .await?;
        Ok(Work::ok(
            rows,
            format!("wrote {} ({rows} rows), pruned {pruned}", target.display()),
        ))
    }

    async fn prepare(
        &self,
        now: Timestamp,
        prefix: &str,
        suffix: &str,
    ) -> Result<Option<(PathBuf, PathBuf)>, IngestError> {
        let directory = &self.config.snapshot_dir;
        let date = now.to_datetime().date_naive();
        let target = directory.join(format!("{prefix}{date}{suffix}"));
        if target.exists() {
            return Ok(None);
        }
        tokio::fs::create_dir_all(directory).await?;
        let partial = directory.join(format!("{prefix}{date}{suffix}{PARTIAL_SUFFIX}"));
        remove(&partial).await?;
        Ok(Some((target, partial)))
    }
}

async fn remove(path: &Path) -> Result<(), IngestError> {
    let removed = match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.is_dir() => tokio::fs::remove_dir_all(path).await,
        Ok(_) => tokio::fs::remove_file(path).await,
        Err(error) => Err(error),
    };
    match removed {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

async fn prune(
    directory: &Path,
    prefix: &str,
    suffix: &str,
    keep: usize,
) -> Result<usize, IngestError> {
    let mut snapshots = Vec::new();
    let mut entries = tokio::fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(prefix) && name.ends_with(suffix) && !name.ends_with(PARTIAL_SUFFIX) {
            snapshots.push(entry.path());
        }
    }
    snapshots.sort();
    let excess = snapshots.len().saturating_sub(keep);
    for path in &snapshots[..excess] {
        remove(path).await?;
    }
    Ok(excess)
}
