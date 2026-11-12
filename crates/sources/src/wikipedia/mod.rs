use std::time::Duration;

use chrono::NaiveDate;
use serde::Deserialize;

use crate::http::{HttpClient, RateLimit, SourceError};

mod membership;
mod wikitext;

pub use membership::{RELIABLE_CHANGES_PER_YEAR, SP500_MEMBERS, Sp500History, normalize_sp500};

pub const BASE_URL: &str = "https://en.wikipedia.org";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 1,
    per: Duration::from_secs(1),
    cooldown: Duration::from_secs(60),
};
pub const SP500_CONSTITUENTS: &str = "List of S&P 500 companies";
pub const SP500_CHANGES: &str = "Historical components of the S&P 500";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WikipediaConfig {
    pub user_agent: String,
    pub base_url: String,
}

#[derive(Clone)]
pub struct WikipediaClient {
    http: HttpClient,
    base_url: String,
}

#[derive(Deserialize)]
struct Parsed {
    parse: Page,
}

#[derive(Deserialize)]
struct Page {
    wikitext: String,
}

impl WikipediaClient {
    pub fn new(config: WikipediaConfig) -> Result<Self, SourceError> {
        Ok(Self {
            http: HttpClient::new(&config.user_agent, RATE_LIMIT)?,
            base_url: config.base_url.trim_end_matches('/').to_owned(),
        })
    }

    pub async fn sp500(&self, today: NaiveDate) -> Result<Sp500History, SourceError> {
        let constituents = self.wikitext(SP500_CONSTITUENTS).await?;
        let changes = self.wikitext(SP500_CHANGES).await?;
        normalize_sp500(&constituents, &changes, today)
    }

    async fn wikitext(&self, title: &str) -> Result<String, SourceError> {
        let query = [
            ("action", "parse"),
            ("page", title),
            ("prop", "wikitext"),
            ("format", "json"),
            ("formatversion", "2"),
            ("redirects", "1"),
        ];
        let url = format!("{}/w/api.php", self.base_url);
        let body = self
            .http
            .fetch(self.http.client().get(url).query(&query))
            .await?;
        let parsed: Parsed = serde_json::from_slice(&body)
            .map_err(|error| SourceError::Decode(format!("{title}: {error}")))?;
        Ok(parsed.parse.wikitext)
    }
}
