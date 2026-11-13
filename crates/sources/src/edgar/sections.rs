use std::collections::BTreeSet;
use std::sync::LazyLock;

use buizz_core::market::Section;
use regex::Regex;

pub const SPLITTER_VERSION: u32 = 1;

const ANNUAL_ITEMS: [&str; 23] = [
    "1", "1A", "1B", "1C", "2", "3", "4", "5", "6", "7", "7A", "8", "9", "9A", "9B", "9C", "10",
    "11", "12", "13", "14", "15", "16",
];
const QUARTERLY_ITEMS: [&str; 11] = [
    "I-1", "I-2", "I-3", "I-4", "II-1", "II-1A", "II-2", "II-3", "II-4", "II-5", "II-6",
];
const LONG_SECTION: u64 = 500;
const LONG_SCORE: i64 = 100;
const ITEM_SCORE: i64 = 1;
const SKIPPED_HEADING_PENALTY: i64 = 100;
const PART_HEADING_CHARS: usize = 120;

static ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*(?i:items?)\s+(\d{1,2})([a-cA-C])?(?:\s*[.:\-–—)]|\t|\s*$|\s+[A-Z\["“'(])"#)
        .expect("item heading")
});
static CURRENT_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^\s*(?i:items?)\s*(\d{1,2}\.\d{2}(?:[\s,;&]*(?i:and\s+)?(?i:items?\s*)?\d{1,2}\.\d{2})*)(?:\s*[.:\-–—()]|\t|\s*$|\s+[A-Z\["“'(])"#,
    )
    .expect("current report item heading")
});
static ITEM_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d{1,2}\.\d{2}").expect("item code"));
static SIGNATURES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*signatures?\s*$").expect("signature heading"));
static PART: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*part\s+(ii|i)\b").expect("part heading"));

struct Heading {
    start: u64,
    rank: usize,
}

pub fn split_sections(form: &str, listed: &[String], text: &str) -> Result<Vec<Section>, String> {
    let (base, items): (&str, &[&str]) = match form.trim_end_matches("/A") {
        "8-K" => return split_current_report(listed, text),
        "10-K" => ("10-K", &ANNUAL_ITEMS),
        "10-Q" => ("10-Q", &QUARTERLY_ITEMS),
        other => return Err(format!("no splitter for form {other}")),
    };
    let quarterly = base == "10-Q";
    let mut headings = Vec::new();
    let mut offset = 0u64;
    let mut part = "I";
    for line in text.split_inclusive('\n') {
        if quarterly
            && line.chars().count() <= PART_HEADING_CHARS
            && let Some(found) = PART.captures(line)
        {
            part = if found[1].eq_ignore_ascii_case("ii") {
                "II"
            } else {
                "I"
            };
        }
        if let Some(found) = ITEM.captures(line) {
            let number = found[1].trim_start_matches('0');
            let letter = found
                .get(2)
                .map(|letter| letter.as_str().to_ascii_uppercase())
                .unwrap_or_default();
            let item = if quarterly {
                format!("{part}-{number}{letter}")
            } else {
                format!("{number}{letter}")
            };
            if let Some(rank) = items.iter().position(|known| *known == item) {
                headings.push(Heading {
                    start: offset,
                    rank,
                });
            }
        }
        offset += line.chars().count() as u64;
    }
    let chosen = best_chain(&headings, offset);
    if chosen.is_empty() {
        return Err("no item headings found".to_owned());
    }
    if chosen.len() > 2
        && chosen
            .windows(2)
            .all(|pair| headings[pair[1]].start - headings[pair[0]].start < LONG_SECTION)
    {
        return Err("item headings appear only in a cross-reference index".to_owned());
    }
    Ok(chosen
        .iter()
        .enumerate()
        .map(|(position, &index)| Section {
            key: format!("{base}:{}", items[headings[index].rank]),
            start: headings[index].start,
            end: chosen
                .get(position + 1)
                .map_or(offset, |&next| headings[next].start),
        })
        .collect())
}

fn score(length: u64, skipped: i64) -> i64 {
    let long = if length >= LONG_SECTION {
        LONG_SCORE
    } else {
        0
    };
    long + ITEM_SCORE - SKIPPED_HEADING_PENALTY * skipped
}

fn best_chain(headings: &[Heading], end: u64) -> Vec<usize> {
    let count = headings.len();
    let mut best = vec![0i64; count];
    let mut previous: Vec<Option<usize>> = vec![None; count];
    let mut finished: Option<(i64, usize)> = None;
    for from in 0..count {
        let mut skipped = 0;
        for to in from + 1..count {
            if headings[to].rank > headings[from].rank {
                let candidate =
                    best[from] + score(headings[to].start - headings[from].start, skipped);
                if candidate > best[to] {
                    best[to] = candidate;
                    previous[to] = Some(from);
                }
                skipped += 1;
            }
        }
        let total = best[from] + score(end - headings[from].start, skipped);
        if finished.is_none_or(|(top, _)| total > top) {
            finished = Some((total, from));
        }
    }
    let mut chain = Vec::new();
    let mut cursor = finished.map(|(_, last)| last);
    while let Some(index) = cursor {
        chain.push(index);
        cursor = previous[index];
    }
    chain.reverse();
    chain
}

fn split_current_report(listed: &[String], text: &str) -> Result<Vec<Section>, String> {
    if listed.is_empty() {
        return Err("the filing lists no items".to_owned());
    }
    let mut headings: Vec<(u64, Vec<String>)> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut offset = 0u64;
    for line in text.split_inclusive('\n') {
        if !headings.is_empty() && SIGNATURES.is_match(line) {
            break;
        }
        if let Some(found) = CURRENT_ITEM.captures(line) {
            let codes: Vec<String> = ITEM_CODE
                .find_iter(&found[1])
                .map(|code| code.as_str().trim_start_matches('0').to_owned())
                .filter(|code| seen.insert(code.clone()))
                .collect();
            if !codes.is_empty() {
                headings.push((offset, codes));
            }
        }
        offset += line.chars().count() as u64;
    }
    let sections: Vec<Section> = headings
        .iter()
        .enumerate()
        .flat_map(|(position, (start, codes))| {
            let end = headings.get(position + 1).map_or(offset, |next| next.0);
            codes
                .iter()
                .filter(|code| listed.contains(code))
                .map(move |code| Section {
                    key: format!("8-K:{code}"),
                    start: *start,
                    end,
                })
        })
        .collect();
    if sections.is_empty() {
        return Err(format!(
            "no heading for items {} (found {})",
            listed.join(", "),
            if seen.is_empty() {
                "none".to_owned()
            } else {
                seen.into_iter().collect::<Vec<_>>().join(", ")
            }
        ));
    }
    Ok(sections)
}
