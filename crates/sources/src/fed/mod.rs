use std::time::Duration;

use buizz_core::market::FomcMeeting;
use buizz_core::time::Timestamp;
use chrono::{NaiveDate, NaiveTime};

use crate::http::{HttpClient, RateLimit, SourceError};

pub const BASE_URL: &str = "https://www.federalreserve.gov";
pub const URL_ENV: &str = "FED_URL";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 1,
    per: Duration::from_secs(1),
    cooldown: Duration::from_secs(60),
};
pub const FIRST_YEAR: i32 = 2000;
const STATEMENT_AT_TWO: NaiveDate = NaiveDate::from_ymd_opt(2013, 1, 1).expect("valid date");
const CALENDAR_YEAR_MARKER: &str = " FOMC Meetings</a></h4>";
const CALENDAR_MONTH_MARKER: &str = "fomc-meeting__month";
const CALENDAR_DATE_MARKER: &str = "fomc-meeting__date";
const HISTORY_HEADING: &str = "<h5";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FedConfig {
    pub base_url: String,
}

#[derive(Clone)]
pub struct FedClient {
    http: HttpClient,
    base_url: String,
}

impl FedClient {
    pub fn new(config: FedConfig) -> Result<Self, SourceError> {
        Ok(Self {
            http: HttpClient::new("buizz", RATE_LIMIT)?,
            base_url: config.base_url.trim_end_matches('/').to_owned(),
        })
    }

    pub async fn calendar(&self) -> Result<Vec<FomcMeeting>, SourceError> {
        let page = self.page("/monetarypolicy/fomccalendars.htm").await?;
        parse_calendar(&page)
    }

    pub async fn history(&self, year: i32) -> Result<Vec<FomcMeeting>, SourceError> {
        let page = self
            .page(&format!("/monetarypolicy/fomchistorical{year}.htm"))
            .await?;
        parse_history(&page, year)
    }

    async fn page(&self, path: &str) -> Result<String, SourceError> {
        let url = format!("{}{path}", self.base_url);
        let body = self.http.fetch(self.http.client().get(url)).await?;
        Ok(String::from_utf8_lossy(&body).into_owned())
    }
}

struct Entry<'a> {
    year: i32,
    title: String,
    unscheduled: bool,
    held: bool,
    body: &'a str,
}

fn parse_calendar(page: &str) -> Result<Vec<FomcMeeting>, SourceError> {
    let mut entries = Vec::new();
    for (year_at, _) in page.match_indices(CALENDAR_YEAR_MARKER) {
        let year = page[..year_at]
            .rsplit('>')
            .next()
            .and_then(|text| text.trim().parse::<i32>().ok())
            .ok_or_else(|| decode("a calendar year heading without a year"))?;
        let panel = &page[year_at + CALENDAR_YEAR_MARKER.len()..];
        let panel = &panel[..panel.find(CALENDAR_YEAR_MARKER).unwrap_or(panel.len())];
        let rows: Vec<usize> = panel
            .match_indices(CALENDAR_MONTH_MARKER)
            .map(|(at, _)| at)
            .collect();
        for (index, start) in rows.iter().enumerate() {
            let row = &panel[*start..rows.get(index + 1).copied().unwrap_or(panel.len())];
            let months = element_text(row).ok_or_else(|| decode("a meeting without a month"))?;
            let date_at = row
                .find(CALENDAR_DATE_MARKER)
                .ok_or_else(|| decode("a meeting without a date"))?;
            let days = element_text(&row[date_at..])
                .ok_or_else(|| decode("a meeting without its days"))?;
            entries.push(Entry {
                year,
                title: format!("{months} {days}"),
                unscheduled: unscheduled(days),
                held: false,
                body: row,
            });
        }
    }
    meetings(entries)
}

fn parse_history(page: &str, year: i32) -> Result<Vec<FomcMeeting>, SourceError> {
    let headings: Vec<usize> = page
        .match_indices(HISTORY_HEADING)
        .map(|(at, _)| at)
        .collect();
    let suffix = format!(" - {year}");
    let mut entries = Vec::new();
    for (index, start) in headings.iter().enumerate() {
        let section = &page[*start..headings.get(index + 1).copied().unwrap_or(page.len())];
        let Some(title) = element_text(section).and_then(|title| title.strip_suffix(&suffix))
        else {
            continue;
        };
        entries.push(Entry {
            year,
            title: title.to_owned(),
            unscheduled: unscheduled(title) || title.contains("Conference Call"),
            held: !title.contains("(cancelled)"),
            body: section,
        });
    }
    meetings(entries)
}

fn unscheduled(text: &str) -> bool {
    text.contains("(unscheduled)") || text.contains("(notation vote)")
}

fn meetings(entries: Vec<Entry<'_>>) -> Result<Vec<FomcMeeting>, SourceError> {
    let mut meetings = Vec::new();
    for entry in entries.into_iter().filter(|entry| entry.year >= FIRST_YEAR) {
        let released = statement_date(entry.body);
        if entry.unscheduled {
            meetings.extend(released.map(|date| FomcMeeting {
                date,
                scheduled: false,
                statement_at: None,
            }));
            continue;
        }
        if entry.held && released.is_none() {
            continue;
        }
        let date = last_day(entry.year, &entry.title)
            .ok_or_else(|| decode(&format!("meeting {} {}", entry.title, entry.year)))?;
        meetings.push(FomcMeeting {
            date,
            scheduled: true,
            statement_at: Some(scheduled_statement_at(
                date,
                entry.body.contains(">Press Conference<"),
            )?),
        });
    }
    meetings.sort();
    meetings.dedup();
    Ok(meetings)
}

fn scheduled_statement_at(
    date: NaiveDate,
    press_conference: bool,
) -> Result<Timestamp, SourceError> {
    let (hour, minute) = if date >= STATEMENT_AT_TWO {
        (14, 0)
    } else if press_conference {
        (12, 30)
    } else {
        (14, 15)
    };
    NaiveTime::from_hms_opt(hour, minute, 0)
        .and_then(|time| Timestamp::at_exchange_time(date, time))
        .ok_or_else(|| decode(&format!("statement time on {date}")))
}

fn element_text(html: &str) -> Option<&str> {
    let mut rest = &html[html.find('>')? + 1..];
    while let Some(tag) = rest.strip_prefix('<') {
        rest = &tag[tag.find('>')? + 1..];
    }
    let text = rest[..rest.find('<')?].trim();
    (!text.is_empty()).then_some(text)
}

fn last_day(year: i32, title: &str) -> Option<NaiveDate> {
    let mut month = None;
    let mut day = None;
    for token in title
        .split('(')
        .next()?
        .split(|c: char| c.is_whitespace() || c == '-')
    {
        let token = token.trim_end_matches('*');
        if let Ok(number) = token.parse::<u32>() {
            day = Some(number);
        } else if let Some(number) = token.rsplit('/').next().and_then(month_number) {
            month = Some(number);
        }
    }
    NaiveDate::from_ymd_opt(year, month?, day?)
}

fn month_number(name: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let prefix = name.trim().get(..3)?.to_ascii_lowercase();
    MONTHS
        .iter()
        .position(|month| *month == prefix)
        .and_then(|index| u32::try_from(index + 1).ok())
}

fn statement_date(body: &str) -> Option<NaiveDate> {
    let link = if let Some(at) = body.find(">Statement</a>") {
        &body[body[..at].rfind("href=\"")?..at]
    } else {
        let listed = &body[body.find("Statement:</strong>")?..];
        &listed[..listed.find("</a>")?]
    };
    link.split(|c: char| !c.is_ascii_digit())
        .filter(|digits| digits.len() == 8)
        .find_map(|digits| NaiveDate::parse_from_str(digits, "%Y%m%d").ok())
}

fn decode(what: &str) -> SourceError {
    SourceError::Decode(format!("FOMC calendar: {what}"))
}
