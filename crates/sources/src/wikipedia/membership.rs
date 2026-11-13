use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use buizz_core::market::{IndexHistory, IndexMember, StockIndex, Symbol};
use chrono::{Datelike, NaiveDate};

use super::wikitext::{first_link, table_rows, ticker};
use crate::http::SourceError;

pub const RELIABLE_CHANGES_PER_YEAR: usize = 15;
pub const SP500_MEMBERS: RangeInclusive<usize> = 495..=510;

struct Side {
    key: String,
    symbol: Symbol,
    article: Option<String>,
}

struct Change {
    date: NaiveDate,
    added: Option<Side>,
    removed: Option<Side>,
}

struct Open {
    key: String,
    symbol: Symbol,
    article: Option<String>,
    end: Option<NaiveDate>,
}

fn side(ticker_cell: Option<&String>, security_cell: Option<&String>) -> Option<Side> {
    let symbol = ticker(ticker_cell?)?;
    let article = security_cell.and_then(|cell| first_link(cell));
    Some(Side {
        key: article.clone().unwrap_or_else(|| symbol.clone()),
        symbol: Symbol::new(&symbol),
        article,
    })
}

fn change_date(cell: &str) -> Option<NaiveDate> {
    let text = cell.split("<ref").next()?.trim();
    NaiveDate::parse_from_str(text, "%B %d, %Y")
        .or_else(|_| NaiveDate::parse_from_str(text, "%Y-%m-%d"))
        .ok()
}

fn decode_error(what: &str) -> SourceError {
    SourceError::Decode(format!("the S&P 500 {what} table is missing or empty"))
}

pub struct Sp500History {
    pub history: IndexHistory,
    pub unparsed_rows: usize,
}

fn blank(cell: Option<&String>) -> bool {
    cell.is_none_or(|text| text.trim().is_empty())
}

fn change(cells: &[String]) -> Option<Change> {
    let added = side(cells.get(1), cells.get(2));
    let removed = side(cells.get(3), cells.get(4));
    let unreadable_added = added.is_none() && !blank(cells.get(1));
    let unreadable_removed = removed.is_none() && !blank(cells.get(3));
    if unreadable_added || unreadable_removed || (added.is_none() && removed.is_none()) {
        return None;
    }
    Some(Change {
        date: change_date(cells.first()?)?,
        added,
        removed,
    })
}

pub fn normalize_sp500(
    constituents: &str,
    changes: &str,
    today: NaiveDate,
) -> Result<Sp500History, SourceError> {
    let constituent_rows =
        table_rows(constituents, "constituents").ok_or_else(|| decode_error("constituents"))?;
    let current: Vec<Side> = constituent_rows
        .iter()
        .filter_map(|cells| side(cells.first(), cells.get(1)))
        .collect();
    let change_rows = table_rows(changes, "changes").ok_or_else(|| decode_error("changes"))?;
    let parsed: Vec<Change> = change_rows
        .iter()
        .filter_map(|cells| change(cells))
        .collect();
    let unparsed_rows = constituent_rows.len() - current.len() + change_rows.len() - parsed.len();
    let mut log: Vec<Change> = parsed
        .into_iter()
        .filter(|change| change.date <= today)
        .collect();
    if current.is_empty() || log.is_empty() {
        return Err(decode_error(if current.is_empty() {
            "constituents"
        } else {
            "changes"
        }));
    }
    log.sort_by_key(|change| std::cmp::Reverse(change.date));

    let mut open: Vec<Open> = current
        .into_iter()
        .map(|member| Open {
            key: member.key,
            symbol: member.symbol,
            article: member.article,
            end: None,
        })
        .collect();
    let mut sizes = vec![(today, open.len())];
    let mut members = Vec::new();
    for day in log.chunk_by(|a, b| a.date == b.date) {
        for (change, added) in day
            .iter()
            .filter_map(|change| Some((change, change.added.as_ref()?)))
        {
            let found = open
                .iter()
                .position(|member| member.key == added.key && member.symbol == added.symbol)
                .or_else(|| open.iter().position(|member| member.key == added.key))
                .or_else(|| open.iter().position(|member| member.symbol == added.symbol));
            if let Some(index) = found {
                let member = open.swap_remove(index);
                members.push(IndexMember {
                    index: StockIndex::Sp500,
                    symbol: member.symbol,
                    start: Some(change.date),
                    end: member.end,
                    article: member.article.or_else(|| added.article.clone()),
                });
            }
        }
        for (change, removed) in day
            .iter()
            .filter_map(|change| Some((change, change.removed.as_ref()?)))
        {
            let present = open
                .iter()
                .any(|member| member.key == removed.key && member.symbol == removed.symbol);
            if !present {
                open.push(Open {
                    key: removed.key.clone(),
                    symbol: removed.symbol.clone(),
                    article: removed.article.clone(),
                    end: Some(change.date),
                });
            }
        }
        let eve = day[0].date.pred_opt().unwrap_or(day[0].date);
        sizes.push((eve, open.len()));
    }
    members.extend(open.into_iter().map(|member| IndexMember {
        index: StockIndex::Sp500,
        symbol: member.symbol,
        start: None,
        end: member.end,
        article: member.article,
    }));
    members.sort_by(|a, b| (&a.symbol, a.start).cmp(&(&b.symbol, b.start)));
    let reliable_from = reliable_from(&log, today);
    let checked_from = reliable_from.unwrap_or(today);
    if let Some((date, size)) = sizes
        .into_iter()
        .find(|(date, size)| *date >= checked_from && !SP500_MEMBERS.contains(size))
    {
        return Err(SourceError::Decode(format!(
            "the parsed S&P 500 has {size} members on {date}, outside {}..={}",
            SP500_MEMBERS.start(),
            SP500_MEMBERS.end()
        )));
    }
    Ok(Sp500History {
        history: IndexHistory {
            index: StockIndex::Sp500,
            members,
            changes: u32::try_from(log.len()).unwrap_or(u32::MAX),
            reliable_from,
        },
        unparsed_rows,
    })
}

fn reliable_from(log: &[Change], today: NaiveDate) -> Option<NaiveDate> {
    let mut per_year: BTreeMap<i32, usize> = BTreeMap::new();
    for change in log {
        *per_year.entry(change.date.year()).or_default() += 1;
    }
    let last_full_year = today.year() - 1;
    let first_year = *per_year.keys().next()?;
    let mut reliable = None;
    for year in (first_year..=last_full_year).rev() {
        if per_year.get(&year).copied().unwrap_or(0) < RELIABLE_CHANGES_PER_YEAR {
            break;
        }
        reliable = Some(year);
    }
    reliable.and_then(|year| NaiveDate::from_ymd_opt(year, 1, 1))
}
