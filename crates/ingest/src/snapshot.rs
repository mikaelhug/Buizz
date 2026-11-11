use std::path::Path;

use buizz_core::time::Timestamp;

use crate::fetch::Work;
use crate::{Ingest, IngestError};

const PREFIX: &str = "state-";
const SUFFIX: &str = ".db";
const PARTIAL_SUFFIX: &str = ".db.partial";

impl Ingest {
    pub(crate) async fn state_snapshot(&self, now: Timestamp) -> Result<Work, IngestError> {
        let directory = &self.config.snapshot_dir;
        let date = now.to_datetime().date_naive();
        let target = directory.join(format!("{PREFIX}{date}{SUFFIX}"));
        if target.exists() {
            return Ok(Work::noop(format!("snapshot for {date} already exists")));
        }
        tokio::fs::create_dir_all(directory).await?;
        let partial = directory.join(format!("{PREFIX}{date}{PARTIAL_SUFFIX}"));
        match tokio::fs::remove_file(&partial).await {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
        self.state.snapshot(&partial).await?;
        tokio::fs::rename(&partial, &target).await?;
        let pruned = prune(directory, self.config.snapshot_keep).await?;
        Ok(Work::ok(
            1,
            format!("wrote {}, pruned {pruned}", target.display()),
        ))
    }
}

async fn prune(directory: &Path, keep: usize) -> Result<usize, IngestError> {
    let mut snapshots = Vec::new();
    let mut entries = tokio::fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(PREFIX) && name.ends_with(SUFFIX) {
            snapshots.push(entry.path());
        }
    }
    snapshots.sort();
    let excess = snapshots.len().saturating_sub(keep);
    for path in &snapshots[..excess] {
        tokio::fs::remove_file(path).await?;
    }
    Ok(excess)
}
