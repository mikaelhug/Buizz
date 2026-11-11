use std::collections::{BTreeMap, HashMap};

use buizz_core::market::{IndexHistory, IndexMember, StockIndex, Symbol};
use chrono::{Datelike, NaiveDate};

use super::wikitext::{first_link, table_rows, ticker};
use crate::http::SourceError;

pub const RELIABLE_CHANGES_PER_YEAR: usize = 15;

struct Side {
    key: String,
    symbol: Symbol,
}

struct Change {
    date: NaiveDate,
    added: Option<Side>,
    removed: Option<Side>,
}

struct Open {
    key: String,
    symbol: Symbol,
    end: Option<NaiveDate>,
}

fn side(ticker_cell: Option<&String>, security_cell: Option<&String>) -> Option<Side> {
    let symbol = ticker(ticker_cell?)?;
    let key = security_cell
        .and_then(|cell| first_link(cell))
        .unwrap_or_else(|| symbol.clone());
    Some(Side {
        key,
        symbol: Symbol::new(&symbol),
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

pub fn normalize_sp500(
    constituents: &str,
    changes: &str,
    today: NaiveDate,
) -> Result<IndexHistory, SourceError> {
    let current: Vec<Side> = table_rows(constituents, "constituents")
        .ok_or_else(|| decode_error("constituents"))?
        .iter()
        .filter_map(|cells| side(cells.first(), cells.get(1)))
        .collect();
    let mut log: Vec<Change> = table_rows(changes, "changes")
        .ok_or_else(|| decode_error("changes"))?
        .iter()
        .filter_map(|cells| {
            Some(Change {
                date: change_date(cells.first()?)?,
                added: side(cells.get(1), cells.get(2)),
                removed: side(cells.get(3), cells.get(4)),
            })
        })
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

    let mut known: HashMap<String, Vec<Symbol>> = HashMap::new();
    let mut open: Vec<Open> = Vec::new();
    for member in current {
        known
            .entry(member.key.clone())
            .or_default()
            .push(member.symbol.clone());
        open.push(Open {
            key: member.key,
            symbol: member.symbol,
            end: None,
        });
    }
    let mut members = Vec::new();
    for change in &log {
        if let Some(added) = &change.added {
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
                });
            }
        }
        if let Some(removed) = &change.removed {
            let symbols = known.entry(removed.key.clone()).or_default();
            let symbol = match symbols.as_slice() {
                [only] if !symbols.contains(&removed.symbol) => only.clone(),
                _ => removed.symbol.clone(),
            };
            if !symbols.contains(&symbol) {
                symbols.push(symbol.clone());
            }
            let present = open
                .iter()
                .any(|member| member.key == removed.key && member.symbol == symbol);
            if !present {
                open.push(Open {
                    key: removed.key.clone(),
                    symbol,
                    end: Some(change.date),
                });
            }
        }
    }
    members.extend(open.into_iter().map(|member| IndexMember {
        index: StockIndex::Sp500,
        symbol: member.symbol,
        start: None,
        end: member.end,
    }));
    members.sort_by(|a, b| (&a.symbol, a.start).cmp(&(&b.symbol, b.start)));
    Ok(IndexHistory {
        index: StockIndex::Sp500,
        members,
        changes: u32::try_from(log.len()).unwrap_or(u32::MAX),
        reliable_from: reliable_from(&log, today),
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
