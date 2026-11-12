use std::collections::BTreeSet;

use buizz_core::market::{Company, DataSource, Dataset, EarningsRelease, Symbol};
use buizz_core::time::Timestamp;
use buizz_sources::edgar::{EARNINGS_FORMS, EdgarClient};
use buizz_sources::http::SourceError;

use crate::facts::NO_CIK;
use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError};

impl Ingest {
    pub(crate) async fn filings(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(edgar) = &self.sources.edgar else {
            return Ok(Work::unconfigured("SEC EDGAR"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Edgar).await? {
            return Ok(work);
        }
        let map_key = FetchKey::new(DataSource::Edgar, Dataset::Companies, None, None);
        let tickers = match edgar.ticker_map().await {
            Ok(tickers) => tickers,
            Err(error) => return self.source_failure(now, &map_key, &error).await,
        };
        let (due, more) = self
            .companies_due(now, Dataset::Companies, &tickers, &EARNINGS_FORMS)
            .await?;
        if due.is_empty() {
            return Ok(Work::noop("no company has a new filing"));
        }
        let stored = self.market.companies().await?;
        let backfilled: BTreeSet<Symbol> = stored
            .iter()
            .map(|company| company.symbol.clone())
            .collect();
        let mut batch = Batch::default();
        let mut without_cik = 0;
        let mut more = more;
        for symbol in due {
            if self.stopping() {
                more = true;
                break;
            }
            let key = FetchKey::new(DataSource::Edgar, Dataset::Companies, Some(&symbol), None);
            let Some(cik) = tickers.get(&symbol).copied() else {
                self.record_rejection(now, &key, NO_CIK).await?;
                without_cik += 1;
                continue;
            };
            let backfill = !backfilled.contains(&symbol);
            match self.company_filings(edgar, &symbol, cik, backfill).await {
                Ok((company, releases)) => {
                    let fetched = releases.len() + 1;
                    let mut rows = self.new_releases(&symbol, releases).await?;
                    if !stored.contains(&company) {
                        rows += self
                            .market
                            .upsert_companies(DataSource::Edgar, vec![company])
                            .await?
                            .rows;
                    }
                    self.record_fetched(now, &key, fetched).await?;
                    batch.success(rows);
                }
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    if batch.source_error(symbol.as_str(), &error) {
                        break;
                    }
                }
            }
        }
        if more {
            batch.more_remains("companies");
        }
        let mut work = batch.finish("filings");
        if without_cik > 0 {
            work.message
                .push_str(&format!(" ({without_cik} tickers without a CIK)"));
        }
        Ok(work)
    }

    async fn company_filings(
        &self,
        edgar: &EdgarClient,
        symbol: &Symbol,
        cik: u32,
        backfill: bool,
    ) -> Result<(Company, Vec<EarningsRelease>), SourceError> {
        let submissions = edgar.submissions(symbol, cik).await?;
        let mut releases = submissions.releases;
        if backfill {
            let older = submissions
                .older_pages
                .iter()
                .filter(|page| page.filing_to >= self.config.history_start);
            for page in older {
                releases.extend(edgar.filings_page(symbol, cik, page).await?);
            }
        }
        Ok((submissions.company, releases))
    }

    async fn new_releases(
        &self,
        symbol: &Symbol,
        releases: Vec<EarningsRelease>,
    ) -> Result<u64, IngestError> {
        let stored = self.market.earnings_accessions(symbol.clone()).await?;
        let fresh: Vec<EarningsRelease> = releases
            .into_iter()
            .filter(|release| {
                release.filed >= self.config.history_start && !stored.contains(&release.accession)
            })
            .collect();
        if fresh.is_empty() {
            return Ok(0);
        }
        Ok(self
            .market
            .upsert_earnings_releases(DataSource::Edgar, fresh)
            .await?
            .rows)
    }
}
