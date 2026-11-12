use std::time::Duration;

use buizz_core::market::{DataSource, Dataset, NewsArticle};
use buizz_core::time::Timestamp;
use buizz_sources::alpaca::NEWS_HISTORY_START;
use buizz_sources::alpaca_stream::{StreamConfig, spawn_news};
use chrono::{NaiveDate, TimeDelta};
use tokio::task::JoinHandle;

use crate::fetch::{Batch, FetchKey, Work};
use crate::{Ingest, IngestError, day_start};

const REQUESTS_PER_RUN: usize = 600;
const SETTLED_AFTER: TimeDelta = TimeDelta::hours(1);
const WINDOW_DAYS: i64 = 7;
const QUIET_WINDOW_DAYS: i64 = 448;
const RECORD_EVERY: Duration = Duration::from_secs(60);

impl Ingest {
    pub(crate) async fn news(&self, now: Timestamp) -> Result<Work, IngestError> {
        let Some(alpaca) = &self.sources.alpaca else {
            return Ok(Work::unconfigured("Alpaca"));
        };
        if let Some(work) = self.backoff_work(now, DataSource::Alpaca).await? {
            return Ok(work);
        }
        let read_through = self
            .market
            .coverage()
            .await?
            .into_iter()
            .find(|row| {
                row.dataset == Dataset::News
                    && row.source == DataSource::Alpaca
                    && row.symbol.is_none()
            })
            .map(|row| row.last);
        let mut first = read_through
            .map_or(NEWS_HISTORY_START.max(self.config.history_start), |last| {
                last + TimeDelta::days(1)
            });
        let settled = |day: NaiveDate| day_start(day + TimeDelta::days(1)) + SETTLED_AFTER <= now;
        let mut batch = Batch::default();
        let mut requests = 0;
        let mut span = WINDOW_DAYS;
        while settled(first) && !self.stopping() {
            if requests >= REQUESTS_PER_RUN {
                batch.more_remains("news weeks");
                break;
            }
            let last = (1..span)
                .map(|offset| first + TimeDelta::days(offset))
                .take_while(|day| settled(*day))
                .last()
                .unwrap_or(first);
            let key = FetchKey::new(DataSource::Alpaca, Dataset::News, None, Some((first, last)));
            let (from, to) = (day_start(first), day_start(last + TimeDelta::days(1)));
            let mut articles = Vec::new();
            let mut token = None;
            let failure = loop {
                requests += 1;
                match alpaca.news_page(from, to, token).await {
                    Ok(page) => {
                        articles.extend(page.articles);
                        match page.next {
                            Some(next) => token = Some(next),
                            None => break None,
                        }
                    }
                    Err(error) => break Some(error),
                }
            };
            if let Some(error) = failure {
                self.record_failure(now, &key, &error).await?;
                batch.source_error(&first.to_string(), &error);
                break;
            }
            let fetched = articles.len();
            span = if fetched == 0 {
                (span * 2).min(QUIET_WINDOW_DAYS)
            } else {
                WINDOW_DAYS
            };
            let summary = self
                .market
                .append_news(DataSource::Alpaca, articles, Some((first, last)))
                .await?;
            self.record_fetched(now, &key, fetched).await?;
            batch.success(summary.rows);
            first = last + TimeDelta::days(1);
        }
        Ok(batch.finish("news"))
    }

    pub fn record_news(&self, stream: Option<StreamConfig>) -> Option<JoinHandle<()>> {
        let stream = stream.filter(|_| self.config.enabled)?;
        let market = self.market.clone();
        Some(tokio::spawn(async move {
            let mut articles = spawn_news(stream);
            let mut every = tokio::time::interval(RECORD_EVERY);
            let mut pending: Vec<NewsArticle> = Vec::new();
            loop {
                tokio::select! {
                    article = articles.recv() => match article {
                        Some(article) => pending.push(article),
                        None => return,
                    },
                    _ = every.tick() => {
                        if pending.is_empty() {
                            continue;
                        }
                        let received = std::mem::take(&mut pending);
                        if let Err(error) = market
                            .append_news(DataSource::AlpacaStream, received, None)
                            .await
                        {
                            tracing::error!(%error, "recording news failed");
                        }
                    }
                }
            }
        }))
    }
}
