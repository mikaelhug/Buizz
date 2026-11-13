use std::collections::BTreeSet;

use buizz_core::market::{Company, DataSource, Dataset, EarningsRelease, Filing, Symbol};
use buizz_core::time::Timestamp;
use buizz_sources::edgar::{EARNINGS_FORMS, EdgarClient, Filings};
use buizz_sources::http::SourceError;

use crate::facts::NO_CIK;
use crate::fetch::{Batch, FetchKey, Work, read};
use crate::{Ingest, IngestError};

impl Ingest {
    pub(crate) async fn filings(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(edgar) = &self.sources.edgar else {
            return Ok(Work::unconfigured("SEC EDGAR"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Edgar).await? {
            return Ok(work);
        }
        let map_key = FetchKey::new(DataSource::Edgar, Dataset::CurrentReports, None, None);
        let tickers = match edgar.ticker_map().await {
            Ok(tickers) => tickers,
            Err(error) => return self.source_failure(now, &map_key, &error).await,
        };
        let (due, more) = self
            .companies_due(
                now,
                Dataset::CurrentReports,
                &tickers,
                &EARNINGS_FORMS,
                &BTreeSet::new(),
            )
            .await?;
        if due.is_empty() {
            return Ok(Work::noop("no company has a new filing"));
        }
        let stored = self.market.companies().await?;
        let mut batch = Batch::default();
        let mut without_cik = 0;
        let mut more = more;
        for symbol in due {
            if self.stopping() {
                more = true;
                break;
            }
            let key = FetchKey::new(
                DataSource::Edgar,
                Dataset::CurrentReports,
                Some(&symbol),
                None,
            );
            let Some(cik) = tickers.get(&symbol).copied() else {
                self.record_rejection(now, &key, NO_CIK).await?;
                without_cik += 1;
                continue;
            };
            let backfill = !self
                .latest_fetch(DataSource::Edgar, Dataset::CurrentReports, &symbol)
                .await?
                .is_some_and(|fetch| read(&fetch));
            match self.company_filings(edgar, &symbol, cik, backfill).await {
                Ok((company, filings)) => {
                    let fetched = filings.reports.len() + 1;
                    let mut rows = self.new_releases(&symbol, filings.releases).await?;
                    rows += self.new_reports(&symbol, filings.reports).await?;
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
    ) -> Result<(Company, Filings), SourceError> {
        let submissions = edgar.submissions(symbol, cik).await?;
        let mut filings = submissions.filings;
        if backfill {
            let older = submissions
                .older_pages
                .iter()
                .filter(|page| page.filing_to >= self.config.history_start);
            for page in older {
                let page = edgar.filings_page(symbol, cik, page).await?;
                filings.releases.extend(page.releases);
                filings.reports.extend(page.reports);
            }
        }
        Ok((submissions.company, filings))
    }

    async fn new_reports(&self, symbol: &Symbol, reports: Vec<Filing>) -> Result<u64, IngestError> {
        let stored = self
            .market
            .current_report_accessions(symbol.clone())
            .await?;
        let fresh: Vec<Filing> = reports
            .into_iter()
            .filter(|report| {
                report.filed >= self.config.history_start && !stored.contains(&report.accession)
            })
            .collect();
        if fresh.is_empty() {
            return Ok(0);
        }
        Ok(self
            .market
            .append_current_reports(DataSource::Edgar, fresh)
            .await?
            .rows)
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
