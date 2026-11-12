use std::path::{Path, PathBuf};

use buizz_core::time::Timestamp;

use crate::fetch::Work;
use crate::{Ingest, IngestError};

const STATE_PREFIX: &str = "state-";
const STATE_SUFFIX: &str = ".db";
const LEGACY_MARKET_PREFIX: &str = "market-";
const MARKET_DIR: &str = "market";
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

    pub(crate) async fn market_snapshot(&self, _now: Timestamp) -> Result<Work, IngestError> {
        let chain = self.config.snapshot_dir.join(MARKET_DIR);
        tokio::fs::create_dir_all(&chain).await?;
        remove_partials(&self.config.snapshot_dir).await?;
        remove_partials(&chain).await?;
        let after = last_exported(&chain).await?;
        let partial = chain.join(format!("{:020}{PARTIAL_SUFFIX}", after + 1));
        tokio::fs::create_dir_all(&partial).await?;
        let Some(export) = self
            .market
            .export_unrecoverable(partial.clone(), after)
            .await?
        else {
            remove(&partial).await?;
            return Ok(Work::noop(format!(
                "no ingest batch since batch {after} to back up"
            )));
        };
        let target = chain.join(format!("{:020}-{:020}", after + 1, export.through));
        tokio::fs::rename(&partial, &target).await?;
        let pruned = prune(&self.config.snapshot_dir, LEGACY_MARKET_PREFIX, "", 0).await?;
        Ok(Work::ok(
            export.rows,
            format!(
                "wrote {} ({} rows), removed {pruned} full copies",
                target.display(),
                export.rows
            ),
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
        remove_partials(directory).await?;
        let partial = directory.join(format!("{prefix}{date}{suffix}{PARTIAL_SUFFIX}"));
        Ok(Some((target, partial)))
    }
}

async fn remove_partials(directory: &Path) -> Result<(), IngestError> {
    let mut entries = tokio::fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        if entry
            .file_name()
            .to_string_lossy()
            .ends_with(PARTIAL_SUFFIX)
        {
            remove(&entry.path()).await?;
        }
    }
    Ok(())
}

async fn last_exported(chain: &Path) -> Result<i64, IngestError> {
    let mut last = 0;
    let mut entries = tokio::fs::read_dir(chain).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(through) = name
            .split_once('-')
            .and_then(|(_, through)| through.parse::<i64>().ok())
        {
            last = last.max(through);
        }
    }
    Ok(last)
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
