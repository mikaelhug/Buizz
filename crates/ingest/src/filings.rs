use std::collections::{BTreeMap, BTreeSet};

use buizz_core::market::{Company, DataSource, Dataset, EarningsRelease, Filing, Symbol};
use buizz_core::ops::FetchStatus;
use buizz_core::time::Timestamp;
use buizz_market::CompanyAccessions;
use buizz_sources::edgar::{COMPANY_FORMS, EdgarClient, Filings, SubmissionsArchive};
use buizz_state::SymbolFetches;
use chrono::NaiveDate;
use futures::StreamExt;

use crate::facts::{COMPANIES_PER_RUN, CompanyScope, NO_CIK};
use crate::fetch::{Batch, EDGAR_IN_FLIGHT, FetchKey, Work, read, waiting};
use crate::{Ingest, IngestError};

const FILINGS_PER_WRITE: usize = 20_000;
const NOT_IN_ARCHIVE: &str = "no filing list in EDGAR's submissions archive";

struct Candidate {
    symbol: Symbol,
    cik: u32,
    backfill: bool,
}

fn backfill(current: Option<&SymbolFetches>, periodic: Option<&SymbolFetches>) -> bool {
    let walked =
        periodic.is_some_and(|fetches| fetches.read || fetches.latest == FetchStatus::Rejected);
    !walked || !current.is_some_and(|fetches| fetches.read)
}

struct Read {
    symbol: Symbol,
    reports: usize,
    periodic: Option<usize>,
}

#[derive(Default)]
struct Pending {
    releases: Vec<EarningsRelease>,
    reports: Vec<Filing>,
    periodic: Vec<Filing>,
    companies: Vec<Company>,
    read: Vec<Read>,
}

struct Stored {
    accessions: CompanyAccessions,
    companies: BTreeMap<Symbol, Company>,
    history_start: NaiveDate,
    pending: Pending,
}

fn fresh<T>(
    history_start: NaiveDate,
    held: &mut BTreeSet<(Symbol, String)>,
    rows: Vec<T>,
    key: impl Fn(&T) -> (NaiveDate, &Symbol, &str),
) -> Vec<T> {
    rows.into_iter()
        .filter(|row| {
            let (filed, symbol, accession) = key(row);
            filed >= history_start && held.insert((symbol.clone(), accession.to_owned()))
        })
        .collect()
}

fn filing_key(filing: &Filing) -> (NaiveDate, &Symbol, &str) {
    (filing.filed, &filing.symbol, &filing.accession)
}

impl Stored {
    fn take(&mut self, read: Option<Read>, company: Company, filings: Filings) {
        let held = &mut self.accessions;
        let start = self.history_start;
        self.pending.releases.extend(fresh(
            start,
            &mut held.earnings_releases,
            filings.releases,
            |release| (release.filed, &release.symbol, &release.accession),
        ));
        self.pending.reports.extend(fresh(
            start,
            &mut held.current_reports,
            filings.reports,
            filing_key,
        ));
        self.pending.periodic.extend(fresh(
            start,
            &mut held.periodic_reports,
            filings.periodic,
            filing_key,
        ));
        if self.companies.get(&company.symbol) != Some(&company) {
            self.companies
                .insert(company.symbol.clone(), company.clone());
            self.pending.companies.push(company);
        }
        self.pending.read.extend(read);
    }
}

impl Pending {
    fn size(&self) -> usize {
        self.releases.len() + self.reports.len() + self.periodic.len() + self.companies.len()
    }
}

impl Ingest {
    pub(crate) async fn filings(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(edgar) = &self.sources.edgar else {
            return Ok(Work::unconfigured("SEC EDGAR"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Edgar).await? {
            return Ok(work);
        }
        let map_key = FetchKey::new(DataSource::Edgar, Dataset::CurrentReports, None, None);
        let scope = match edgar.ticker_map().await {
            Ok(tickers) => self.company_scope(&tickers).await?,
            Err(error) => return self.source_failure(now, &map_key, &error).await,
        };
        let read_through = self.filing_lists_read_through().await?;
        tokio::fs::create_dir_all(&self.config.snapshot_dir).await?;
        let mut batch = Batch::default();
        let archive = match edgar
            .submissions_archive(read_through, &self.config.snapshot_dir)
            .await
        {
            Ok(archive) => archive,
            Err(error) => {
                let key = FetchKey::new(DataSource::Edgar, Dataset::CurrentReports, None, None);
                self.record_failure(now, &key, &error).await?;
                if batch.source_error("submissions archive", &error) {
                    return Ok(batch.finish("filings"));
                }
                None
            }
        };
        let mut candidates = Vec::new();
        if archive.is_none() {
            candidates = self.candidates(now, &scope, read_through).await?;
            if candidates.is_empty() && !batch.attempted() {
                return Ok(Work::noop("no company has a new filing"));
            }
        }
        let mut stored = Stored {
            accessions: self.market.company_accessions().await?,
            companies: self
                .market
                .companies()
                .await?
                .into_iter()
                .map(|company| (company.symbol.clone(), company))
                .collect(),
            history_start: self.config.history_start,
            pending: Pending::default(),
        };
        let mut without_cik = 0;
        if let Some(archive) = &archive {
            without_cik = self
                .read_archive(now, archive, &scope, &mut stored, &mut batch)
                .await?;
            self.flush(now, &mut stored.pending, &mut batch).await?;
            if self.stopping() {
                batch.more_remains("companies");
                return Ok(batch.finish("filings"));
            }
            self.market
                .record_filing_lists(DataSource::Edgar, archive.filed_through)
                .await?;
            candidates = self
                .candidates(now, &scope, Some(archive.filed_through))
                .await?;
        }
        let more = candidates.len() > COMPANIES_PER_RUN;
        candidates.truncate(COMPANIES_PER_RUN);
        self.read_companies(now, edgar, candidates, &mut stored, &mut batch)
            .await?;
        self.flush(now, &mut stored.pending, &mut batch).await?;
        if more || self.stopping() {
            batch.more_remains("companies");
        }
        let mut work = batch.finish("filings");
        if without_cik > 0 {
            work.message
                .push_str(&format!(" ({without_cik} tickers without a CIK)"));
        }
        Ok(work)
    }

    async fn filing_lists_read_through(&self) -> Result<Option<NaiveDate>, IngestError> {
        Ok(self
            .market
            .coverage()
            .await?
            .into_iter()
            .find(|row| {
                row.dataset == Dataset::CurrentReports
                    && row.source == DataSource::Edgar
                    && row.symbol.is_none()
            })
            .map(|row| row.last))
    }

    async fn read_archive(
        &self,
        now: Timestamp,
        archive: &SubmissionsArchive,
        scope: &CompanyScope,
        stored: &mut Stored,
        batch: &mut Batch,
    ) -> Result<usize, IngestError> {
        let current = self
            .state
            .symbol_fetches(DataSource::Edgar, Dataset::CurrentReports)
            .await?;
        let periodic = self
            .state
            .symbol_fetches(DataSource::Edgar, Dataset::PeriodicReports)
            .await?;
        let mut without_cik = 0;
        for symbol in &scope.symbols {
            if self.stopping() {
                break;
            }
            let fetches = current.get(symbol);
            let rejected = fetches.is_some_and(|fetches| fetches.latest == FetchStatus::Rejected);
            let key = FetchKey::new(
                DataSource::Edgar,
                Dataset::CurrentReports,
                Some(symbol),
                None,
            );
            let Some(cik) = scope.ciks.get(symbol).copied() else {
                without_cik += 1;
                if !rejected {
                    self.record_rejection(now, &key, NO_CIK).await?;
                }
                continue;
            };
            let backfill = backfill(fetches, periodic.get(symbol));
            let since = backfill.then_some(self.config.history_start);
            match archive.company_filings(symbol, cik, since).await {
                Ok(Some((company, filings))) => {
                    let settled = fetches.is_some_and(|SymbolFetches { latest, .. }| {
                        matches!(latest, FetchStatus::Ok | FetchStatus::Empty)
                    });
                    let read = (backfill || !settled).then(|| Read {
                        symbol: symbol.clone(),
                        reports: filings.reports.len() + 1,
                        periodic: backfill.then_some(filings.periodic.len()),
                    });
                    stored.take(read, company, filings);
                    batch.success(0);
                }
                Ok(None) => {
                    if !rejected {
                        self.record_rejection(now, &key, NOT_IN_ARCHIVE).await?;
                    }
                }
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    batch.source_error(symbol.as_str(), &error);
                }
            }
            if stored.pending.size() >= FILINGS_PER_WRITE {
                self.flush(now, &mut stored.pending, batch).await?;
            }
        }
        Ok(without_cik)
    }

    async fn candidates(
        &self,
        now: Timestamp,
        scope: &CompanyScope,
        read_through: Option<NaiveDate>,
    ) -> Result<Vec<Candidate>, IngestError> {
        let filed = self.market.latest_sec_filings(&COMPANY_FORMS).await?;
        let current = self
            .state
            .symbol_fetches(DataSource::Edgar, Dataset::CurrentReports)
            .await?;
        let periodic = self
            .state
            .symbol_fetches(DataSource::Edgar, Dataset::PeriodicReports)
            .await?;
        let mut candidates = Vec::new();
        for symbol in &scope.symbols {
            let Some(cik) = scope.ciks.get(symbol).copied() else {
                continue;
            };
            let fetches = current.get(symbol);
            if fetches.is_some_and(|fetches| fetches.latest == FetchStatus::Rejected) {
                continue;
            }
            let backfill = backfill(fetches, periodic.get(symbol));
            let last_filed = filed
                .get(&cik)
                .copied()
                .filter(|filed| read_through.is_some_and(|through| *filed > through));
            if !backfill && last_filed.is_none() {
                continue;
            }
            let latest = self
                .latest_fetch(DataSource::Edgar, Dataset::CurrentReports, symbol)
                .await?;
            let fresh = latest.is_some_and(|fetch| {
                waiting(&fetch, now)
                    || (!backfill
                        && read(&fetch)
                        && last_filed.is_some_and(|filed| filed < fetch.at.exchange_date()))
            });
            if !fresh {
                candidates.push(Candidate {
                    symbol: symbol.clone(),
                    cik,
                    backfill,
                });
            }
        }
        Ok(candidates)
    }

    async fn read_companies(
        &self,
        now: Timestamp,
        edgar: &EdgarClient,
        candidates: Vec<Candidate>,
        stored: &mut Stored,
        batch: &mut Batch,
    ) -> Result<(), IngestError> {
        let history_start = self.config.history_start;
        let mut reads = futures::stream::iter(candidates)
            .map(|candidate| async move {
                let since = candidate.backfill.then_some(history_start);
                let filings = edgar
                    .company_filings(&candidate.symbol, candidate.cik, since)
                    .await;
                (candidate, filings)
            })
            .buffered(EDGAR_IN_FLIGHT);
        while let Some((
            Candidate {
                symbol, backfill, ..
            },
            filings,
        )) = reads.next().await
        {
            if self.stopping() {
                break;
            }
            match filings {
                Ok((company, filings)) => {
                    let read = Read {
                        symbol: symbol.clone(),
                        reports: filings.reports.len() + 1,
                        periodic: backfill.then_some(filings.periodic.len()),
                    };
                    stored.take(Some(read), company, filings);
                    batch.success(0);
                }
                Err(error) => {
                    let key = FetchKey::new(
                        DataSource::Edgar,
                        Dataset::CurrentReports,
                        Some(&symbol),
                        None,
                    );
                    self.record_failure(now, &key, &error).await?;
                    if batch.source_error(symbol.as_str(), &error) {
                        break;
                    }
                }
            }
            if stored.pending.size() >= FILINGS_PER_WRITE {
                self.flush(now, &mut stored.pending, batch).await?;
            }
        }
        Ok(())
    }

    async fn flush(
        &self,
        now: Timestamp,
        pending: &mut Pending,
        batch: &mut Batch,
    ) -> Result<(), IngestError> {
        let Pending {
            releases,
            reports,
            periodic,
            companies,
            read,
        } = std::mem::take(pending);
        let mut rows = 0;
        if !releases.is_empty() {
            rows += self
                .market
                .upsert_earnings_releases(DataSource::Edgar, releases)
                .await?
                .rows;
        }
        if !reports.is_empty() {
            rows += self
                .market
                .append_current_reports(DataSource::Edgar, reports)
                .await?
                .rows;
        }
        if !periodic.is_empty() {
            rows += self
                .market
                .append_periodic_reports(DataSource::Edgar, periodic)
                .await?
                .rows;
        }
        if !companies.is_empty() {
            rows += self
                .market
                .upsert_companies(DataSource::Edgar, companies)
                .await?
                .rows;
        }
        for Read {
            symbol,
            reports,
            periodic,
        } in read
        {
            let key = |dataset| FetchKey::new(DataSource::Edgar, dataset, Some(&symbol), None);
            self.record_fetched(now, &key(Dataset::CurrentReports), reports)
                .await?;
            if let Some(periodic) = periodic {
                self.record_fetched(now, &key(Dataset::PeriodicReports), periodic)
                    .await?;
            }
        }
        if rows > 0 {
            batch.success(rows);
        }
        Ok(())
    }
}
