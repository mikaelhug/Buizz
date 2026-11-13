use buizz_core::market::{DataSource, DocumentSections, UnsplitDocument};
use buizz_sources::edgar::{SPLITTER_VERSION, split_sections};

use crate::fetch::{Batch, Work};
use crate::{Ingest, IngestError};

const DOCUMENTS_PER_BATCH: usize = 200;
const BATCHES_PER_RUN: usize = 25;

impl Ingest {
    pub(crate) async fn filing_sections(&self) -> Result<Work, IngestError> {
        if self.market.text_volumes().volumes().is_empty() {
            return Ok(Work::unconfigured("text volumes (BUIZZ_TEXT_DIRS)"));
        }
        let mut batch = Batch::default();
        for _ in 0..BATCHES_PER_RUN {
            if self.stopping() {
                break;
            }
            let documents = self
                .market
                .unsplit_documents(SPLITTER_VERSION, DOCUMENTS_PER_BATCH)
                .await?;
            if documents.is_empty() {
                return Ok(batch.finish("filing sections"));
            }
            let split = tokio::task::spawn_blocking(move || {
                documents.into_iter().map(split).collect::<Vec<_>>()
            })
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))?;
            batch.success(
                self.market
                    .append_document_sections(DataSource::Edgar, split)
                    .await?
                    .rows,
            );
        }
        batch.more_remains("documents");
        Ok(batch.finish("filing sections"))
    }
}

fn split(document: UnsplitDocument) -> DocumentSections {
    DocumentSections {
        outcome: match &document.text {
            Some(text) => split_sections(&document.kind, text),
            None => Err("the document has no text".to_owned()),
        },
        doc_id: document.doc_id,
        content_hash: document.content_hash,
        published_at: document.published_at,
        splitter_version: SPLITTER_VERSION,
    }
}
