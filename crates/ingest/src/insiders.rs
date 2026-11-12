use std::collections::BTreeSet;

use buizz_core::market::{DataSource, Dataset, InsiderFiling, Symbol};
use buizz_core::time::Timestamp;
use buizz_sources::edgar::{EdgarClient, INSIDER_FORMS, OwnershipFiling};
use buizz_sources::http::SourceError;

use crate::facts::NO_CIK;
use crate::fetch::{Batch, FetchKey, Work, read};
use crate::{Ingest, IngestError};

const DOCUMENTS_PER_RUN: usize = 3000;
const NOT_FOUND: u16 = 404;

struct Collected {
    filings: Vec<InsiderFiling>,
    outcome: Result<bool, SourceError>,
}

impl Ingest {
    pub(crate) async fn insider_trades(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(edgar) = &self.sources.edgar else {
            return Ok(Work::unconfigured("SEC EDGAR"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Edgar).await? {
            return Ok(work);
        }
        let map_key = FetchKey::new(DataSource::Edgar, Dataset::InsiderTrades, None, None);
        let tickers = match edgar.ticker_map().await {
            Ok(tickers) => tickers,
            Err(error) => return self.source_failure(now, &map_key, &error).await,
        };
        let (due, mut more) = self
            .companies_due(now, Dataset::InsiderTrades, &tickers, &INSIDER_FORMS)
            .await?;
        if due.is_empty() {
            return Ok(Work::noop("no company has a new insider filing"));
        }
        let mut batch = Batch::default();
        let mut budget = DOCUMENTS_PER_RUN;
        let mut without_cik = 0;
        for symbol in due {
            if self.stopping() || budget == 0 {
                more = true;
                break;
            }
            let key = FetchKey::new(
                DataSource::Edgar,
                Dataset::InsiderTrades,
                Some(&symbol),
                None,
            );
            let Some(cik) = tickers.get(&symbol).copied() else {
                self.record_rejection(now, &key, NO_CIK).await?;
                without_cik += 1;
                continue;
            };
            let backfilled = self
                .latest_fetch(DataSource::Edgar, Dataset::InsiderTrades, &symbol)
                .await?
                .is_some_and(|fetch| read(&fetch));
            let stored = self.market.insider_accessions(symbol.clone()).await?;
            let collected = self
                .collect_insider_filings(edgar, &symbol, cik, !backfilled, &stored, &mut budget)
                .await;
            let fetched = collected.filings.len();
            let rows = if collected.filings.is_empty() {
                0
            } else {
                self.market
                    .append_insider_filings(DataSource::Edgar, collected.filings)
                    .await?
                    .rows
            };
            match collected.outcome {
                Ok(true) => {
                    self.record_fetched(now, &key, fetched).await?;
                    batch.success(rows);
                }
                Ok(false) => {
                    more = true;
                    batch.success(rows);
                }
                Err(error) => {
                    batch.success(rows);
                    self.record_failure(now, &key, &error).await?;
                    if batch.source_error(symbol.as_str(), &error) {
                        break;
                    }
                }
            }
        }
        if more {
            batch.more_remains("insider filings");
        }
        let mut work = batch.finish("insider trades");
        if without_cik > 0 {
            work.message
                .push_str(&format!(" ({without_cik} tickers without a CIK)"));
        }
        Ok(work)
    }

    async fn collect_insider_filings(
        &self,
        edgar: &EdgarClient,
        symbol: &Symbol,
        cik: u32,
        backfill: bool,
        stored: &BTreeSet<String>,
        budget: &mut usize,
    ) -> Collected {
        let mut filings = Vec::new();
        let listed = match self.ownership_filings(edgar, symbol, cik, backfill).await {
            Ok(listed) => listed,
            Err(error) => {
                return Collected {
                    filings,
                    outcome: Err(error),
                };
            }
        };
        let fresh = listed.into_iter().filter(|filing| {
            filing.filed >= self.config.history_start && !stored.contains(&filing.accession)
        });
        for filing in fresh {
            if self.stopping() || *budget == 0 {
                return Collected {
                    filings,
                    outcome: Ok(false),
                };
            }
            if !filing.is_xml() {
                filings.push(without_document(symbol, cik, &filing));
                continue;
            }
            *budget -= 1;
            match edgar.ownership(symbol, cik, &filing).await {
                Ok(parsed) => filings.push(parsed),
                Err(
                    error @ (SourceError::Decode(_)
                    | SourceError::Status {
                        status: NOT_FOUND, ..
                    }),
                ) => {
                    tracing::warn!(%error, accession = filing.accession, "unreadable ownership document");
                    filings.push(without_document(symbol, cik, &filing));
                }
                Err(error) => {
                    return Collected {
                        filings,
                        outcome: Err(error),
                    };
                }
            }
        }
        Collected {
            filings,
            outcome: Ok(true),
        }
    }

    async fn ownership_filings(
        &self,
        edgar: &EdgarClient,
        symbol: &Symbol,
        cik: u32,
        backfill: bool,
    ) -> Result<Vec<OwnershipFiling>, SourceError> {
        let submissions = edgar.submissions(symbol, cik).await?;
        let mut listed = submissions.filings.ownership;
        if backfill {
            let older = submissions
                .older_pages
                .iter()
                .filter(|page| page.filing_to >= self.config.history_start);
            for page in older {
                listed.extend(edgar.filings_page(symbol, cik, page).await?.ownership);
            }
        }
        Ok(listed)
    }
}

fn without_document(symbol: &Symbol, cik: u32, filing: &OwnershipFiling) -> InsiderFiling {
    InsiderFiling {
        symbol: symbol.clone(),
        issuer_cik: cik,
        accession: filing.accession.clone(),
        form: filing.form.clone(),
        filed: filing.filed,
        accepted_at: filing.accepted_at,
        owners: Vec::new(),
        transactions: Vec::new(),
    }
}
