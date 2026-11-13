use std::collections::{BTreeMap, BTreeSet};

use buizz_core::market::{DataSource, Dataset, InsiderFiling, InsiderPlan, InsiderReport, Symbol};
use buizz_core::time::Timestamp;
use buizz_sources::edgar::{EdgarClient, FIRST_INSIDER_QUARTER, INSIDER_FORMS, OwnershipFiling};
use buizz_sources::http::SourceError;
use chrono::{NaiveDate, TimeDelta};
use futures::StreamExt;

use crate::delistings::{next_quarter, quarter_of, quarter_span};
use crate::facts::NO_CIK;
use crate::fetch::{Batch, EDGAR_IN_FLIGHT, FetchKey, Work, read};
use crate::{Ingest, IngestError, day_start};

const DOCUMENTS_PER_RUN: usize = 15_000;
const NOT_FOUND: u16 = 404;
const NO_DATA_SETS: &str = "SEC's insider data set page lists no quarter";
const PLAN_CHECKBOX_FROM: NaiveDate = NaiveDate::from_ymd_opt(2023, 4, 1).expect("a valid date");

pub(crate) fn insider_quarters_due(
    history_start: NaiveDate,
    today: NaiveDate,
) -> ((i32, u32), (i32, u32)) {
    let first = FIRST_INSIDER_QUARTER.max(quarter_of(history_start));
    let ended = quarter_of(quarter_span(quarter_of(today)).0 - TimeDelta::days(1));
    (first, ended)
}

#[derive(Default)]
struct Collected {
    filings: Vec<InsiderFiling>,
    plans: Vec<InsiderPlan>,
}

enum Reading {
    Read(InsiderReport),
    Unreadable,
    Failed(SourceError),
}

impl Ingest {
    pub(crate) async fn insider_trades(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(edgar) = &self.sources.edgar else {
            return Ok(Work::unconfigured("SEC EDGAR"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Edgar).await? {
            return Ok(work);
        }
        let mut batch = Batch::default();
        if !self.read_insider_quarters(now, edgar, &mut batch).await? {
            return Ok(batch.finish("insider trades"));
        }
        let map_key = FetchKey::new(DataSource::Edgar, Dataset::InsiderTrades, None, None);
        let tickers = match edgar.ticker_map().await {
            Ok(tickers) => tickers,
            Err(error) => return self.source_failure(now, &map_key, &error).await,
        };
        let mut unread: BTreeMap<Symbol, Vec<InsiderPlan>> = BTreeMap::new();
        for plan in self
            .market
            .unread_plans(day_start(PLAN_CHECKBOX_FROM))
            .await?
        {
            unread.entry(plan.symbol.clone()).or_default().push(plan);
        }
        let pending: BTreeSet<Symbol> = unread.keys().cloned().collect();
        let (due, mut more) = self
            .companies_due(
                now,
                Dataset::InsiderTrades,
                &tickers,
                &INSIDER_FORMS,
                &pending,
            )
            .await?;
        if due.is_empty() {
            return Ok(if batch.attempted() {
                batch.finish("insider trades")
            } else {
                Work::noop("no company has a new insider filing")
            });
        }
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
            let unread = unread.remove(&symbol).unwrap_or_default();
            let since = unread
                .iter()
                .map(|plan| plan.filed)
                .chain((!backfilled).then_some(self.config.history_start))
                .min();
            let mut collected = Collected::default();
            let outcome = match self.ownership_filings(edgar, &symbol, cik, since).await {
                Ok(listed) => {
                    let staged = self
                        .market
                        .insider_reports(
                            listed
                                .iter()
                                .map(|filing| filing.accession.clone())
                                .collect(),
                        )
                        .await?;
                    self.collect_insider_filings(
                        edgar,
                        (&symbol, cik),
                        (&listed, &staged),
                        (&stored, unread),
                        &mut budget,
                        &mut collected,
                    )
                    .await
                }
                Err(error) => Err(error),
            };
            let fetched = collected.filings.len() + collected.plans.len();
            let mut rows = 0;
            if !collected.filings.is_empty() {
                rows += self
                    .market
                    .append_insider_filings(DataSource::Edgar, collected.filings)
                    .await?
                    .rows;
            }
            if !collected.plans.is_empty() {
                rows += self
                    .market
                    .append_insider_plans(DataSource::Edgar, collected.plans)
                    .await?
                    .rows;
            }
            match outcome {
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

    async fn read_insider_quarters(
        &self,
        now: Timestamp,
        edgar: &EdgarClient,
        batch: &mut Batch,
    ) -> Result<bool, IngestError> {
        let (first, ended) = insider_quarters_due(self.config.history_start, now.exchange_date());
        let read = self.market.insider_quarters().await?;
        let mut quarter = first;
        while quarter <= ended && read.contains(&quarter_span(quarter).0) {
            quarter = next_quarter(quarter);
        }
        if quarter > ended {
            return Ok(true);
        }
        let page = FetchKey::new(DataSource::Edgar, Dataset::InsiderDataSets, None, None);
        let listed = match edgar.insider_quarters().await {
            Ok(listed) => listed,
            Err(error) => {
                self.record_failure(now, &page, &error).await?;
                batch.source_error("insider data sets", &error);
                return Ok(false);
            }
        };
        let listed: Vec<_> = listed
            .into_iter()
            .filter(|listed| listed.quarter >= first)
            .collect();
        let Some(latest) = listed.last() else {
            self.record_rejection(now, &page, NO_DATA_SETS).await?;
            batch.failure("insider data sets", &NO_DATA_SETS);
            return Ok(true);
        };
        let through = (quarter_span(first).0, quarter_span(latest.quarter).1);
        let page = FetchKey::new(
            DataSource::Edgar,
            Dataset::InsiderDataSets,
            None,
            Some(through),
        );
        self.record_fetched(now, &page, listed.len()).await?;
        for listed in listed
            .iter()
            .filter(|listed| !read.contains(&quarter_span(listed.quarter).0))
        {
            if self.stopping() {
                batch.more_remains("insider data set quarters");
                return Ok(false);
            }
            let span = quarter_span(listed.quarter);
            match edgar.insider_quarter(listed).await {
                Ok(reports) => {
                    let summary = self
                        .market
                        .append_insider_quarter(DataSource::Edgar, span, reports)
                        .await?;
                    batch.success(summary.rows);
                }
                Err(error) => {
                    let key = FetchKey::new(
                        DataSource::Edgar,
                        Dataset::InsiderDataSets,
                        None,
                        Some(span),
                    );
                    self.record_failure(now, &key, &error).await?;
                    let (year, number) = listed.quarter;
                    batch.source_error(&format!("insider data set {year}Q{number}"), &error);
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    async fn collect_insider_filings(
        &self,
        edgar: &EdgarClient,
        (symbol, cik): (&Symbol, u32),
        (listed, staged): (&[OwnershipFiling], &BTreeMap<String, InsiderReport>),
        (stored, unread): (&BTreeSet<String>, Vec<InsiderPlan>),
        budget: &mut usize,
        collected: &mut Collected,
    ) -> Result<bool, SourceError> {
        let mut documents: Vec<&OwnershipFiling> = Vec::new();
        for filing in listed.iter().filter(|filing| {
            filing.filed >= self.config.history_start && !stored.contains(&filing.accession)
        }) {
            match staged.get(&filing.accession) {
                Some(report) => collected
                    .filings
                    .push(filing.dated(symbol, cik, report.clone())),
                None => documents.push(filing),
            }
        }
        let mut affordable = 0;
        for filing in &documents {
            if *budget == 0 {
                break;
            }
            *budget -= usize::from(filing.is_xml());
            affordable += 1;
        }
        let chosen: Vec<OwnershipFiling> = documents[..affordable]
            .iter()
            .map(|filing| (*filing).clone())
            .collect();
        let mut reads = futures::stream::iter(chosen)
            .map(|filing| async move {
                let reading = self.read_ownership(edgar, cik, &filing).await;
                (filing, reading)
            })
            .buffered(EDGAR_IN_FLIGHT);
        while let Some((filing, reading)) = reads.next().await {
            if self.stopping() {
                return Ok(false);
            }
            match reading {
                Reading::Read(report) => collected.filings.push(filing.dated(symbol, cik, report)),
                Reading::Unreadable => collected.filings.push(filing.dated(
                    symbol,
                    cik,
                    without_document(&filing, cik),
                )),
                Reading::Failed(error) => return Err(error),
            }
        }
        drop(reads);
        if affordable < documents.len() {
            return Ok(false);
        }
        let by_accession: BTreeMap<&str, &OwnershipFiling> = listed
            .iter()
            .map(|filing| (filing.accession.as_str(), filing))
            .collect();
        for plan in unread {
            if let Some(report) = staged.get(&plan.accession) {
                collected.plans.push(InsiderPlan {
                    plan_10b5_1: report.plan_10b5_1,
                    ..plan
                });
                continue;
            }
            if self.stopping() || *budget == 0 {
                return Ok(false);
            }
            let plan_10b5_1 = match by_accession.get(plan.accession.as_str()) {
                Some(filing) => {
                    *budget -= usize::from(filing.is_xml());
                    match self.read_ownership(edgar, cik, filing).await {
                        Reading::Read(report) => report.plan_10b5_1,
                        Reading::Unreadable => None,
                        Reading::Failed(error) => return Err(error),
                    }
                }
                None => None,
            };
            collected.plans.push(InsiderPlan {
                plan_10b5_1,
                ..plan
            });
        }
        Ok(true)
    }

    async fn read_ownership(
        &self,
        edgar: &EdgarClient,
        cik: u32,
        filing: &OwnershipFiling,
    ) -> Reading {
        if !filing.is_xml() {
            return Reading::Unreadable;
        }
        match edgar.ownership(cik, filing).await {
            Ok(parsed) => Reading::Read(parsed),
            Err(
                error @ (SourceError::Decode(_)
                | SourceError::Status {
                    status: NOT_FOUND, ..
                }),
            ) => {
                tracing::warn!(%error, accession = filing.accession, "unreadable ownership document");
                Reading::Unreadable
            }
            Err(error) => Reading::Failed(error),
        }
    }

    async fn ownership_filings(
        &self,
        edgar: &EdgarClient,
        symbol: &Symbol,
        cik: u32,
        since: Option<NaiveDate>,
    ) -> Result<Vec<OwnershipFiling>, SourceError> {
        let submissions = edgar.submissions(symbol, cik).await?;
        let mut listed = submissions.filings.ownership;
        if let Some(since) = since {
            let older = submissions
                .older_pages
                .iter()
                .filter(|page| page.filing_to >= since);
            for page in older {
                listed.extend(edgar.filings_page(symbol, cik, page).await?.ownership);
            }
        }
        Ok(listed)
    }
}

fn without_document(filing: &OwnershipFiling, cik: u32) -> InsiderReport {
    InsiderReport {
        accession: filing.accession.clone(),
        issuer_cik: cik,
        plan_10b5_1: None,
        owners: Vec::new(),
        transactions: Vec::new(),
    }
}
