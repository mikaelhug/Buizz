use std::sync::LazyLock;
use std::time::Duration;

use buizz_core::market::EarningsInsight;
use buizz_core::time::Timestamp;
use chrono::{DateTime, NaiveDate};
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
const TOOL_TIMEOUT: Duration = Duration::from_secs(60);
const NAMED_REPORT: &str = ".*[Ee]arnings_?[Ii]nsight.*";

static HUBSPOT_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)/EarningsInsight(\d?)_(\d{2})(\d{2})(\d{2})([A-Z]?)\.pdf$")
        .expect("valid pattern")
});
static PLONE_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)/earningsinsight_?(\d{1,2})\.(\d{1,2})\.(\d{2})/?$").expect("valid pattern")
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
        Some(Self {
            report: format!("{date}{version}"),
            issue_date: date,
            url: path,
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
    let (text, stamped) = read_pdf(&body)
        .await
        .map_err(|error| SourceError::Decode(format!("{}: {error}", issue.url)))?;
    Ok(EarningsInsight {
        published: stamped.into_iter().fold(issue.issue_date, NaiveDate::max),
        report: issue.report,
        issue_date: issue.issue_date,
        url: issue.url,
        retrieved_from,
        digest: blake3::hash(&body).to_hex().to_string(),
        text,
    })
}

pub async fn read_pdf(body: &[u8]) -> Result<(String, Vec<NaiveDate>), String> {
    let file = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
    std::fs::write(file.path(), body).map_err(|error| error.to_string())?;
    let path = file.path().as_os_str();
    let text = poppler(
        "pdftotext",
        &[
            "-layout".as_ref(),
            "-enc".as_ref(),
            "UTF-8".as_ref(),
            path,
            "-".as_ref(),
        ],
    )
    .await?;
    if text.trim().is_empty() {
        return Err("the PDF has no text".to_owned());
    }
    let info = poppler("pdfinfo", &["-isodates".as_ref(), path]).await?;
    let stamped = info
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            ["CreationDate", "ModDate"]
                .contains(&key)
                .then(|| stamp_date(value.trim()))?
        })
        .collect();
    Ok((text, stamped))
}

async fn poppler(tool: &str, args: &[&std::ffi::OsStr]) -> Result<String, String> {
    let run = tokio::process::Command::new(tool)
        .args(args)
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(TOOL_TIMEOUT, run)
        .await
        .map_err(|_| format!("{tool} took longer than {TOOL_TIMEOUT:?}"))?
        .map_err(|error| format!("{tool}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{tool} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn stamp_date(value: &str) -> Option<NaiveDate> {
    let value = value
        .strip_suffix('Z')
        .map_or_else(|| value.to_owned(), |utc| format!("{utc}+00"));
    let moment = DateTime::parse_from_str(&value, "%Y-%m-%dT%H:%M:%S%#z").ok()?;
    Some(Timestamp::from_datetime(moment.to_utc())?.exchange_date())
}
