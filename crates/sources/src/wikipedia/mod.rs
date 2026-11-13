use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{Map, Value};

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
pub const REST_URL: &str = "https://wikimedia.org";
pub const REST_URL_ENV: &str = "WIKIMEDIA_URL";
pub const REST_RATE_LIMIT: RateLimit = RateLimit {
    requests: 5,
    per: Duration::from_secs(1),
    cooldown: Duration::from_secs(60),
};
pub const FIRST_PAGE_VIEWS: NaiveDate = NaiveDate::from_ymd_opt(2015, 7, 1).expect("valid date");
pub const MOVED_REDIRECTS: &str = "Category:Redirects from moves";
const TITLES_PER_QUERY: usize = 50;
const NO_VIEWS: u16 = 404;
pub const SP500_CONSTITUENTS: &str = "List of S&P 500 companies";
pub const SP500_CHANGES: &str = "Historical components of the S&P 500";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WikipediaConfig {
    pub user_agent: String,
    pub base_url: String,
    pub rest_url: String,
}

#[derive(Clone)]
pub struct WikipediaClient {
    http: HttpClient,
    rest: HttpClient,
    base_url: String,
    rest_url: String,
}

#[derive(Deserialize)]
struct Query {
    #[serde(default)]
    query: QueryPages,
    #[serde(default, rename = "continue")]
    more: Option<Map<String, Value>>,
}

#[derive(Default, Deserialize)]
struct QueryPages {
    #[serde(default)]
    normalized: Vec<Rename>,
    #[serde(default)]
    redirects: Vec<Rename>,
    #[serde(default)]
    pages: Vec<QueryPage>,
}

#[derive(Deserialize)]
struct Rename {
    from: String,
    to: String,
    #[serde(default)]
    tofragment: Option<String>,
}

#[derive(Deserialize)]
struct QueryPage {
    title: String,
    #[serde(default)]
    missing: bool,
    #[serde(default)]
    redirects: Vec<Redirect>,
    #[serde(default)]
    categories: Vec<Value>,
}

#[derive(Deserialize)]
struct Redirect {
    title: String,
    #[serde(default)]
    fragment: Option<String>,
}

#[derive(Deserialize)]
struct DailyViews {
    items: Vec<DayViews>,
}

#[derive(Deserialize)]
struct DayViews {
    timestamp: String,
    views: u64,
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
            rest: HttpClient::new(&config.user_agent, REST_RATE_LIMIT)?,
            base_url: config.base_url.trim_end_matches('/').to_owned(),
            rest_url: config.rest_url.trim_end_matches('/').to_owned(),
        })
    }

    pub async fn article_titles(
        &self,
        articles: &[String],
    ) -> Result<BTreeMap<String, BTreeSet<String>>, SourceError> {
        let mut titles = BTreeMap::new();
        for chunk in articles.chunks(TITLES_PER_QUERY) {
            let mut normalized: BTreeMap<String, String> = BTreeMap::new();
            let mut redirected: BTreeMap<String, Rename> = BTreeMap::new();
            let mut aliases: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            let joined = chunk.join("|");
            self.query(
                &[
                    ("titles", joined.as_str()),
                    ("redirects", "1"),
                    ("prop", "redirects"),
                    ("rdnamespace", "0"),
                    ("rdprop", "title|fragment"),
                    ("rdlimit", "max"),
                ],
                |pages| {
                    for rename in pages.normalized {
                        normalized.insert(rename.from, rename.to);
                    }
                    for rename in pages.redirects {
                        redirected.insert(rename.from.clone(), rename);
                    }
                    for page in pages.pages.into_iter().filter(|page| !page.missing) {
                        aliases.entry(page.title).or_default().extend(
                            page.redirects
                                .into_iter()
                                .filter(|redirect| redirect.fragment.is_none())
                                .map(|redirect| redirect.title),
                        );
                    }
                },
            )
            .await?;
            let candidates: Vec<String> = aliases
                .values()
                .flatten()
                .chain(redirected.keys())
                .cloned()
                .collect();
            let mut moved = BTreeSet::new();
            for batch in candidates.chunks(TITLES_PER_QUERY) {
                let joined = batch.join("|");
                self.query(
                    &[
                        ("titles", joined.as_str()),
                        ("prop", "categories"),
                        ("clcategories", MOVED_REDIRECTS),
                        ("cllimit", "max"),
                    ],
                    |pages| {
                        moved.extend(
                            pages
                                .pages
                                .into_iter()
                                .filter(|page| !page.categories.is_empty())
                                .map(|page| page.title),
                        );
                    },
                )
                .await?;
            }
            for article in chunk {
                let linked = normalized.get(article).unwrap_or(article);
                let names: BTreeSet<String> = match redirected.get(linked) {
                    Some(rename) if rename.tofragment.is_none() && moved.contains(linked) => {
                        with_moves(&rename.to, &aliases, &moved)
                    }
                    Some(_) => BTreeSet::from([linked.clone()]),
                    None if aliases.contains_key(linked) => with_moves(linked, &aliases, &moved),
                    None => continue,
                };
                titles.insert(article.clone(), names);
            }
        }
        Ok(titles)
    }

    pub async fn daily_views(
        &self,
        title: &str,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<(NaiveDate, u64)>, SourceError> {
        let mut url = reqwest::Url::parse(&format!(
            "{}/api/rest_v1/metrics/pageviews/per-article/en.wikipedia/all-access/user/",
            self.rest_url
        ))
        .map_err(|error| SourceError::Decode(error.to_string()))?;
        url.path_segments_mut()
            .map_err(|()| SourceError::Decode("the Wikimedia URL has no path".to_owned()))?
            .pop_if_empty()
            .push(&title.replace(' ', "_"))
            .push("daily")
            .push(&from.format("%Y%m%d").to_string())
            .push(&to.format("%Y%m%d").to_string());
        let body = match self.rest.fetch(self.rest.client().get(url)).await {
            Ok(body) => body,
            Err(SourceError::Status {
                status: NO_VIEWS, ..
            }) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let daily: DailyViews = serde_json::from_slice(&body)
            .map_err(|error| SourceError::Decode(format!("{title}: {error}")))?;
        daily
            .items
            .into_iter()
            .map(|day| {
                let date = day
                    .timestamp
                    .get(..8)
                    .and_then(|digits| NaiveDate::parse_from_str(digits, "%Y%m%d").ok())
                    .ok_or_else(|| {
                        SourceError::Decode(format!("{title}: timestamp {:?}", day.timestamp))
                    })?;
                Ok((date, day.views))
            })
            .collect()
    }

    async fn query(
        &self,
        parameters: &[(&str, &str)],
        mut read: impl FnMut(QueryPages),
    ) -> Result<(), SourceError> {
        let url = format!("{}/w/api.php", self.base_url);
        let mut more: Map<String, Value> = Map::new();
        loop {
            let mut request = self
                .http
                .client()
                .get(&url)
                .query(&[
                    ("action", "query"),
                    ("format", "json"),
                    ("formatversion", "2"),
                ])
                .query(parameters);
            for (key, value) in &more {
                request = request.query(&[(key.as_str(), value.as_str().unwrap_or_default())]);
            }
            let body = self.http.fetch(request).await?;
            let page: Query = serde_json::from_slice(&body)
                .map_err(|error| SourceError::Decode(format!("query: {error}")))?;
            read(page.query);
            match page.more {
                Some(next) => more = next,
                None => return Ok(()),
            }
        }
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

fn with_moves(
    canonical: &str,
    aliases: &BTreeMap<String, BTreeSet<String>>,
    moved: &BTreeSet<String>,
) -> BTreeSet<String> {
    aliases
        .get(canonical)
        .into_iter()
        .flatten()
        .filter(|alias| moved.contains(*alias))
        .cloned()
        .chain([canonical.to_owned()])
        .collect()
}
