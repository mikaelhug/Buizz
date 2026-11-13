use std::collections::{BTreeMap, BTreeSet};

use buizz_core::market::{DataSource, Dataset, IndexHistory, StockIndex, TitleViews};
use buizz_core::time::Timestamp;
use buizz_core::visibility::page_views_published;
use buizz_sources::wikipedia::FIRST_PAGE_VIEWS;
use chrono::TimeDelta;

use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError};

const TITLES_PER_RUN: usize = 1500;
pub(crate) const NO_PAGE: &str = "";

pub(crate) fn company_articles(history: &IndexHistory) -> BTreeSet<String> {
    history
        .members
        .iter()
        .filter(|member| member.end.is_none_or(|end| end > FIRST_PAGE_VIEWS))
        .filter_map(|member| member.article.clone())
        .collect()
}

impl Ingest {
    pub(crate) async fn page_views(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(wikipedia) = &self.sources.wikipedia else {
            return Ok(Work::unconfigured("Wikipedia"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Wikipedia).await? {
            return Ok(work);
        }
        let Some(history) = self.market.index_history(StockIndex::Sp500).await? else {
            return Ok(Work::waiting(
                "page views: the S&P 500 membership is not stored yet",
            ));
        };
        let today = now.exchange_date();
        let through = today - TimeDelta::days(1);
        let key = FetchKey::new(DataSource::Wikipedia, Dataset::PageViews, None, None);
        let articles = company_articles(&history);
        let mut titles = self.market.article_titles().await?;
        let unresolved: Vec<String> = articles
            .iter()
            .filter(|article| !titles.contains_key(*article))
            .cloned()
            .collect();
        let mut resolved = if unresolved.is_empty() {
            BTreeMap::new()
        } else {
            match wikipedia.article_titles(&unresolved).await {
                Ok(resolved) => resolved,
                Err(error) => return self.source_failure(now, &key, &error).await,
            }
        };
        for article in unresolved {
            resolved
                .entry(article)
                .or_insert_with(|| BTreeSet::from([NO_PAGE.to_owned()]));
        }
        titles.extend(resolved.clone());
        let read_through = self.market.page_views_through().await?;
        let mut batch = Batch::default();
        let mut requests = 0;
        'articles: for article in &articles {
            let Some(names) = titles.get(article) else {
                continue;
            };
            let mut views = Vec::new();
            for title in names.iter().filter(|title| title.as_str() != NO_PAGE) {
                let from = read_through
                    .get(title)
                    .map_or(FIRST_PAGE_VIEWS, |last| *last + TimeDelta::days(1));
                if from > through {
                    continue;
                }
                if self.stopping() {
                    break 'articles;
                }
                if requests == TITLES_PER_RUN {
                    batch.more_remains(format!("titles from {title}"));
                    break 'articles;
                }
                requests += 1;
                match wikipedia.daily_views(title, from, through).await {
                    Ok(days) => views.extend(days.into_iter().map(|(date, count)| TitleViews {
                        title: title.clone(),
                        date,
                        views: count,
                        published: page_views_published(date, today),
                    })),
                    Err(error) => {
                        self.record_failure(now, &key, &error).await?;
                        if batch.source_error(title, &error) {
                            break 'articles;
                        }
                    }
                }
            }
            let new_titles: BTreeMap<String, BTreeSet<String>> = resolved
                .remove(article)
                .map(|names| BTreeMap::from([(article.clone(), names)]))
                .unwrap_or_default();
            if views.is_empty() && new_titles.is_empty() {
                continue;
            }
            let summary = self
                .market
                .append_page_views(DataSource::Wikipedia, new_titles, views)
                .await?;
            batch.success(summary.rows);
        }
        if !resolved.is_empty() {
            self.market
                .append_page_views(DataSource::Wikipedia, resolved, Vec::new())
                .await?;
        }
        self.record_fetched(now, &key, requests).await?;
        Ok(batch.finish("page views"))
    }
}
