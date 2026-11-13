use buizz_core::market::{DataSource, FilingText};
use buizz_core::time::Timestamp;
use buizz_sources::http::SourceError;
use futures::StreamExt;

use crate::fetch::{Batch, EDGAR_IN_FLIGHT, Work};
use crate::{Ingest, IngestError};

const FILINGS_PER_RUN: usize = 15_000;
const STORED_EVERY: usize = 25;
const STORED_BYTES: usize = 128 << 20;
const NOT_FOUND: u16 = 404;

impl Ingest {
    pub(crate) async fn filing_documents(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(edgar) = &self.sources.edgar else {
            return Ok(Work::unconfigured("SEC EDGAR"));
        };
        if self.market.text_volumes().volumes().is_empty() {
            return Ok(Work::unconfigured("text volumes (BUIZZ_TEXT_DIRS)"));
        }
        if let Some(work) = self.backoff_work(now, DataSource::Edgar).await? {
            return Ok(work);
        }
        let candidates = self.market.text_candidates().await?;
        if candidates.is_empty() {
            return Ok(Work::noop("every filing has its documents"));
        }
        let mut left = candidates.len();
        let mut batch = Batch::default();
        let mut read: Vec<FilingText> = Vec::new();
        let mut held = 0;
        let mut reads = futures::stream::iter(candidates.into_iter().take(FILINGS_PER_RUN))
            .map(|candidate| async move {
                let documents = edgar
                    .filing_documents(candidate.cik, &candidate.accession)
                    .await;
                (candidate, documents)
            })
            .buffered(EDGAR_IN_FLIGHT);
        while let Some((candidate, documents)) = reads.next().await {
            if self.stopping() {
                break;
            }
            left -= 1;
            let outcome = match documents {
                Ok(documents) => {
                    held += documents
                        .iter()
                        .map(|document| {
                            document.raw.len() + document.text.as_ref().map_or(0, String::len)
                        })
                        .sum::<usize>();
                    Ok(documents)
                }
                Err(
                    error @ (SourceError::Decode(_)
                    | SourceError::Status {
                        status: NOT_FOUND, ..
                    }),
                ) => {
                    tracing::warn!(%error, accession = candidate.accession, "unreadable filing");
                    Err(error.to_string())
                }
                Err(error) => {
                    if batch.source_error(&candidate.accession, &error) {
                        break;
                    }
                    continue;
                }
            };
            read.push(FilingText {
                cik: candidate.cik,
                accession: candidate.accession,
                form: candidate.form,
                accepted_at: candidate.accepted_at,
                retrieved_at: Timestamp::now(),
                outcome,
            });
            if read.len() == STORED_EVERY || held >= STORED_BYTES {
                batch.success(self.store_filing_texts(std::mem::take(&mut read)).await?);
                held = 0;
            }
        }
        if !read.is_empty() {
            batch.success(self.store_filing_texts(read).await?);
        }
        if left > 0 {
            batch.more_remains(format!("{left} filings"));
        }
        Ok(batch.finish("filing documents"))
    }

    async fn store_filing_texts(&self, filings: Vec<FilingText>) -> Result<u64, IngestError> {
        Ok(self
            .market
            .append_filing_texts(DataSource::Edgar, filings)
            .await?
            .rows)
    }
}
