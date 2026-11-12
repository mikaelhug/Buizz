use std::collections::BTreeMap;

use buizz_core::market::{DataSource, Dataset, EarningsInsight, EstimateFact};
use buizz_core::time::Timestamp;
use buizz_sources::factset::{self, Capture};
use buizz_sources::http::SourceError;

use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError};

const ARCHIVED_PER_RUN: usize = 100;

impl Ingest {
    pub(crate) async fn earnings_insight(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(client) = &self.sources.factset else {
            return Ok(Work::unconfigured("FactSet"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Factset).await? {
            return Ok(work);
        }
        let key = FetchKey::new(DataSource::Factset, Dataset::Estimates, None, None);
        let issue = match client.latest().await {
            Ok(issue) => issue,
            Err(error) => return self.source_failure(now, &key, &error).await,
        };
        let stored = self.market.earnings_insight_reports().await?;
        let mut rows = 0;
        if !stored.contains(&issue.report) {
            let report = match client.report(&issue).await {
                Ok(report) => report,
                Err(error) => return self.source_failure(now, &key, &error).await,
            };
            self.record_fetched(now, &key, 1).await?;
            rows += self.store_report(report).await?;
        }
        rows += self.reparse_earnings_insights().await?;
        Ok(if rows == 0 {
            Work::noop(format!("Earnings Insight {} already stored", issue.report))
        } else {
            Work::ok(
                rows,
                format!("Earnings Insight {}: {rows} rows", issue.report),
            )
        })
    }

    pub(crate) async fn earnings_insight_archive(
        &self,
        now: Timestamp,
    ) -> Result<Work, IngestError> {
        let Some(client) = &self.sources.factset else {
            return Ok(Work::unconfigured("FactSet"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Factset).await? {
            return Ok(work);
        }
        let key = FetchKey::new(DataSource::Factset, Dataset::Estimates, None, None);
        let captures = match client.archive().await {
            Ok(captures) => captures,
            Err(error) => return self.source_failure(now, &key, &error).await,
        };
        let stored = self.market.earnings_insight_reports().await?;
        let missing: Vec<&Capture> = captures
            .iter()
            .filter(|capture| !stored.contains(&capture.issue.report))
            .collect();
        let mut batch = Batch::default();
        for (fetched, capture) in missing.iter().enumerate() {
            if fetched == ARCHIVED_PER_RUN || self.stopping() {
                batch.more_remains(format!("{} reports", missing.len() - fetched));
                break;
            }
            let report = match client.report(&capture.issue).await {
                Err(SourceError::Status { .. } | SourceError::Decode(_)) => {
                    client.archived_report(capture).await
                }
                outcome => outcome,
            };
            match report {
                Ok(report) => batch.success(self.store_report(report).await?),
                Err(error) => {
                    self.record_failure(now, &key, &error).await?;
                    if batch.source_error(&capture.issue.report, &error) {
                        break;
                    }
                }
            }
        }
        self.record_fetched(now, &key, captures.len()).await?;
        let reparsed = self.reparse_earnings_insights().await?;
        if reparsed > 0 {
            batch.success(reparsed);
        }
        Ok(batch.finish("Earnings Insight archive"))
    }

    async fn store_report(&self, report: EarningsInsight) -> Result<u64, IngestError> {
        Ok(self
            .market
            .append_earnings_insight(DataSource::Factset, report)
            .await?
            .rows)
    }

    async fn reparse_earnings_insights(&self) -> Result<u64, IngestError> {
        let reports = self.market.earnings_insights().await?;
        let stored = self.market.earnings_insight_facts().await?;
        let changed = tokio::task::spawn_blocking(move || {
            reports
                .iter()
                .filter_map(|report| {
                    let facts = sorted(factset::parse(report));
                    let known = sorted(stored.get(&report.report).cloned().unwrap_or_default());
                    (facts != known).then(|| (report.report.clone(), facts))
                })
                .collect::<BTreeMap<String, Vec<EstimateFact>>>()
        })
        .await
        .map_err(|error| std::io::Error::other(error.to_string()))?;
        if changed.is_empty() {
            return Ok(0);
        }
        Ok(self
            .market
            .replace_earnings_insight_facts(DataSource::Factset, changed)
            .await?
            .rows)
    }
}

fn sorted(mut facts: Vec<EstimateFact>) -> Vec<EstimateFact> {
    facts.sort_by_key(|fact| (fact.metric, fact.scope, fact.period));
    facts
}
