use std::collections::{BTreeMap, BTreeSet};

use buizz_core::market::{DataSource, PressCandidate, PressExhibit, Symbol};
use buizz_core::time::Timestamp;
use buizz_sources::http::SourceError;
use futures::StreamExt;

use crate::fetch::{Batch, EDGAR_IN_FLIGHT, Work};
use crate::{Ingest, IngestError};

const RELEASES_PER_RUN: usize = 6_000;
const STORED_EVERY: usize = 100;
const NOT_FOUND: u16 = 404;

impl Ingest {
    pub(crate) async fn press_releases(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(edgar) = &self.sources.edgar else {
            return Ok(Work::unconfigured("SEC EDGAR"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Edgar).await? {
            return Ok(work);
        }
        let candidates = self.press_candidates().await?;
        if candidates.is_empty() {
            return Ok(Work::noop("every earnings release has its press release"));
        }
        let mut left = candidates.len();
        let mut batch = Batch::default();
        let mut read: Vec<PressExhibit> = Vec::new();
        let mut reads = futures::stream::iter(candidates.into_iter().take(RELEASES_PER_RUN))
            .map(|candidate| async move {
                let release = edgar
                    .press_release(candidate.cik, &candidate.accession, candidate.accepted_at)
                    .await;
                (candidate, release)
            })
            .buffered(EDGAR_IN_FLIGHT);
        while let Some((candidate, fetched)) = reads.next().await {
            if self.stopping() {
                break;
            }
            left -= 1;
            match fetched {
                Ok(release) => read.push(release),
                Err(
                    error @ (SourceError::Decode(_)
                    | SourceError::Status {
                        status: NOT_FOUND, ..
                    }),
                ) => {
                    tracing::warn!(%error, accession = candidate.accession, "unreadable filing index");
                    read.push(PressExhibit {
                        cik: candidate.cik,
                        accession: candidate.accession,
                        accepted_at: candidate.accepted_at,
                        exhibit: None,
                    });
                }
                Err(error) => {
                    if batch.source_error(&candidate.accession, &error) {
                        break;
                    }
                }
            }
            if read.len() == STORED_EVERY {
                batch.success(self.store_press_releases(std::mem::take(&mut read)).await?);
            }
        }
        if !read.is_empty() {
            batch.success(self.store_press_releases(read).await?);
        }
        if left > 0 {
            batch.more_remains(format!("{left} press releases"));
        }
        Ok(batch.finish("press releases"))
    }

    async fn press_candidates(&self) -> Result<Vec<PressCandidate>, IngestError> {
        let ranks: BTreeMap<Symbol, usize> = self
            .priced_symbols()
            .await?
            .into_iter()
            .map(|priced| (priced.symbol, priced.rank))
            .collect();
        let mut candidates = self.market.press_candidates().await?;
        candidates
            .sort_by_key(|candidate| ranks.get(&candidate.symbol).copied().unwrap_or(usize::MAX));
        let mut seen = BTreeSet::new();
        candidates.retain(|candidate| seen.insert(candidate.accession.clone()));
        Ok(candidates)
    }

    async fn store_press_releases(&self, releases: Vec<PressExhibit>) -> Result<u64, IngestError> {
        Ok(self
            .market
            .append_press_releases(DataSource::Edgar, releases)
            .await?
            .rows)
    }
}
