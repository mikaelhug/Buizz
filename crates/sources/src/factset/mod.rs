use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::LazyLock;
use std::time::Duration;

use buizz_core::market::EarningsInsight;
use buizz_core::time::Timestamp;
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime};
use pdf_extract::{Document, Object, PlainTextOutput};
use regex::Regex;

use crate::http::{HttpClient, RateLimit, SourceError};

mod parse;

pub use parse::parse;

pub const BASE_URL: &str = "https://www.factset.com";
pub const URL_ENV: &str = "FACTSET_URL";
pub const ARCHIVE_URL_ENV: &str = "WAYBACK_URL";
pub const ARCHIVE_URL: &str = "https://web.archive.org";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 20,
    per: Duration::from_secs(60),
    cooldown: Duration::from_secs(60),
};
pub const ARCHIVE_RATE_LIMIT: RateLimit = RateLimit {
    requests: 6,
    per: Duration::from_secs(60),
    cooldown: Duration::from_secs(5 * 60),
};
pub const ARCHIVED_PREFIXES: [&str; 5] = [
    "www.factset.com/websitefiles/PDFs/",
    "insight.factset.com/hubfs/",
    "www.factset.com/hubfs/",
    "go.factset.com/hubfs/",
    "advantage.factset.com/hubfs/",
];
const NAMED_REPORT: &str = ".*[Ee]arnings_?[Ii]nsight.*";

static HUBSPOT_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)/EarningsInsight(\d?)_(\d{2})(\d{2})(\d{2})([A-Z]?)\.pdf$")
        .expect("valid pattern")
});
static PLONE_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)/earningsinsight_?(\d{1,2})\.(\d{1,2})\.(\d{2})/?$").expect("valid pattern")
});
static PDF_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^D:(\d{14})(?:(Z)|([+-])(\d{2})'?(\d{2})?'?)?").expect("valid pattern")
});

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FactsetConfig {
    pub user_agent: String,
    pub base_url: String,
    pub archive_url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Issue {
    pub report: String,
    pub issue_date: NaiveDate,
    pub url: String,
}

impl Issue {
    pub fn from_url(url: &str) -> Option<Self> {
        let path = url.split(['?', '#']).next()?.replace(":80/", "/");
        let (date, version) = if let Some(name) = HUBSPOT_NAME.captures(&path) {
            let number = |index: usize| name[index].parse::<u32>().ok();
            let date = NaiveDate::from_ymd_opt(
                2000 + i32::try_from(number(4)?).ok()?,
                number(2)?,
                number(3)?,
            )?;
            let draft = match &name[1] {
                "" => String::new(),
                number => format!("v{number}"),
            };
            (date, format!("{}{draft}", name[5].to_ascii_uppercase()))
        } else {
            let name = PLONE_NAME.captures(&path)?;
            let number = |index: usize| name[index].parse::<u32>().ok();
            let date = NaiveDate::from_ymd_opt(
                2000 + i32::try_from(number(3)?).ok()?,
                number(1)?,
                number(2)?,
            )?;
            (date, String::new())
        };
        let url = if path.starts_with("http://") {
            path.replacen("http://", "https://", 1)
        } else {
            path
        };
        Some(Self {
            report: format!("{date}{version}"),
            issue_date: date,
            url,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capture {
    pub issue: Issue,
    pub timestamp: String,
}

#[derive(Clone)]
pub struct FactsetClient {
    factset: HttpClient,
    archive: HttpClient,
    config: FactsetConfig,
}

impl FactsetClient {
    pub fn new(config: FactsetConfig) -> Result<Self, SourceError> {
        Ok(Self {
            factset: HttpClient::new(&config.user_agent, RATE_LIMIT)?,
            archive: HttpClient::new(&config.user_agent, ARCHIVE_RATE_LIMIT)?,
            config: FactsetConfig {
                base_url: config.base_url.trim_end_matches('/').to_owned(),
                archive_url: config.archive_url.trim_end_matches('/').to_owned(),
                ..config
            },
        })
    }

    pub async fn latest(&self) -> Result<Issue, SourceError> {
        let request = self
            .factset
            .client()
            .head(format!("{}/earningsinsight", self.config.base_url));
        let (url, _) = self.factset.fetch_located(request).await?;
        Issue::from_url(&url)
            .ok_or_else(|| SourceError::Decode(format!("{url} is not an Earnings Insight")))
    }

    pub async fn archive(&self) -> Result<Vec<Capture>, SourceError> {
        let mut captures = Vec::new();
        for prefix in ARCHIVED_PREFIXES {
            let request = self
                .archive
                .client()
                .get(format!("{}/cdx/search/cdx", self.config.archive_url))
                .query(&[
                    ("url", prefix),
                    ("matchType", "prefix"),
                    ("filter", "statuscode:200"),
                    ("filter", "mimetype:application/pdf"),
                    ("filter", &format!("original:{NAMED_REPORT}")),
                    ("fl", "timestamp,original"),
                    ("collapse", "urlkey"),
                ]);
            let body = self.archive.fetch(request).await?;
            captures.extend(String::from_utf8_lossy(&body).lines().filter_map(|line| {
                let (timestamp, original) = line.split_once(' ')?;
                Some(Capture {
                    issue: Issue::from_url(original)?,
                    timestamp: timestamp.to_owned(),
                })
            }));
        }
        captures.sort_by(|a, b| (&a.issue, &a.timestamp).cmp(&(&b.issue, &b.timestamp)));
        captures.dedup_by(|later, earlier| later.issue.report == earlier.issue.report);
        Ok(captures)
    }

    pub async fn report(&self, issue: &Issue) -> Result<EarningsInsight, SourceError> {
        let (url, body) = self
            .factset
            .fetch_located(self.factset.client().get(&issue.url))
            .await?;
        read_report(issue.clone(), url, body).await
    }

    pub async fn archived_report(&self, capture: &Capture) -> Result<EarningsInsight, SourceError> {
        let url = format!(
            "{}/web/{}id_/{}",
            self.config.archive_url, capture.timestamp, capture.issue.url
        );
        let body = self.archive.fetch(self.archive.client().get(&url)).await?;
        read_report(capture.issue.clone(), url, body).await
    }
}

async fn read_report(
    issue: Issue,
    retrieved_from: String,
    body: Vec<u8>,
) -> Result<EarningsInsight, SourceError> {
    tokio::task::spawn_blocking(move || {
        let (text, created) = read_pdf(&body)
            .map_err(|error| SourceError::Decode(format!("{}: {error}", issue.url)))?;
        Ok(EarningsInsight {
            published: created.into_iter().fold(issue.issue_date, NaiveDate::max),
            report: issue.report,
            issue_date: issue.issue_date,
            url: issue.url,
            retrieved_from,
            digest: blake3::hash(&body).to_hex().to_string(),
            text,
        })
    })
    .await
    .map_err(|error| SourceError::Decode(format!("reading the report panicked: {error}")))?
}

pub fn read_pdf(body: &[u8]) -> Result<(String, Vec<NaiveDate>), String> {
    let document = Document::load_mem(body).map_err(|error| error.to_string())?;
    let mut text = String::new();
    let mut unreadable = Vec::new();
    for page in document.get_pages().into_keys() {
        let mut output = PlainTextOutput::new(&mut text);
        let read = catch_unwind(AssertUnwindSafe(|| {
            pdf_extract::output_doc_page(&document, &mut output, page)
        }));
        if !matches!(read, Ok(Ok(()))) {
            unreadable.push(page);
        }
    }
    if !unreadable.is_empty() {
        tracing::warn!(
            ?unreadable,
            "pages of an Earnings Insight the PDF reader could not read"
        );
    }
    let stamped = ["CreationDate", "ModDate"]
        .into_iter()
        .filter_map(|key| info_date(&document, key))
        .collect();
    Ok((text, stamped))
}

fn info_date(document: &Document, key: &str) -> Option<NaiveDate> {
    let info = document.trailer.get(b"Info").ok()?;
    let info = match info {
        Object::Reference(id) => document.get_dictionary(*id).ok()?,
        Object::Dictionary(dictionary) => dictionary,
        _ => return None,
    };
    let Object::String(bytes, _) = info.get(key.as_bytes()).ok()? else {
        return None;
    };
    let stamp = PDF_DATE.captures(std::str::from_utf8(bytes).ok()?)?;
    let local = NaiveDateTime::parse_from_str(&stamp[1], "%Y%m%d%H%M%S").ok()?;
    let seconds = match (stamp.get(3), stamp.get(4)) {
        (Some(sign), Some(hours)) => {
            let minutes = stamp
                .get(5)
                .map_or(Some(0), |text| text.as_str().parse().ok())?;
            let offset = hours.as_str().parse::<i32>().ok()? * 3600 + minutes * 60;
            if sign.as_str() == "-" {
                -offset
            } else {
                offset
            }
        }
        _ => 0,
    };
    let moment: DateTime<FixedOffset> = local
        .and_local_timezone(FixedOffset::east_opt(seconds)?)
        .single()?;
    Some(Timestamp::from_datetime(moment.to_utc())?.exchange_date())
}
