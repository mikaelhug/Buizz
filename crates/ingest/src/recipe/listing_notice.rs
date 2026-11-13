use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::EVIDENCE_CHARS;

const RULE: &str = r"\d{3,4}(?:\.\d{2}[A-Z]?)?(?:\([A-Za-z0-9]{1,4}\))*";
const LEAD_CHARS: usize = 150;

static EXCHANGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(nyse\s+american|nyse\s+mkt|nyse\s+amex|american\s+stock\s+exchange|new\s+york\s+stock\s+exchange|nyse|nasdaq)\b",
    )
    .expect("a valid exchange pattern")
});

static CITED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)\b(?:rules?|sections?)\s+({RULE}(?:\s*(?:,\s*and|,|and|or)\s*{RULE})*)"
    ))
    .expect("a valid citation pattern")
});

static RULE_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(RULE).expect("a valid rule pattern"));

fn exchange(name: &str) -> &'static str {
    let name = name.to_ascii_lowercase();
    if name.starts_with("nyse ") || name.starts_with("american") {
        "nyse_american"
    } else if name.starts_with("nasdaq") {
        "nasdaq"
    } else {
        "nyse"
    }
}

fn sentence(text: &str, at: Range<usize>) -> Range<usize> {
    let before = &text[..at.start];
    let after = &text[at.end..];
    let start = [
        before.rfind('\n').map(|index| index + 1),
        before.rfind(". ").map(|index| index + 2),
    ]
    .into_iter()
    .flatten()
    .max()
    .unwrap_or(0);
    let end = [after.find('\n'), after.find(". ").map(|index| index + 1)]
        .into_iter()
        .flatten()
        .min()
        .map_or(text.len(), |index| at.end + index);
    start..end
}

fn evidence(text: &str, at: Range<usize>) -> (&str, Range<usize>) {
    let bounds = sentence(text, at.clone());
    let limit = usize::try_from(EVIDENCE_CHARS).unwrap_or(usize::MAX);
    let chars: Vec<(usize, char)> = text[bounds.clone()].char_indices().collect();
    if chars.len() <= limit {
        return (text[bounds.clone()].trim(), bounds);
    }
    let lead = text[bounds.start..at.start].chars().count();
    let first = lead.saturating_sub(LEAD_CHARS).min(chars.len() - limit);
    let start = bounds.start + chars[first].0;
    let end = chars
        .get(first + limit)
        .map_or(bounds.end, |(offset, _)| bounds.start + offset);
    (text[start..end].trim(), start..end)
}

pub(super) fn read(text: &str) -> Value {
    let mut facts: Vec<Value> = Vec::new();
    let mut rules: Vec<String> = Vec::new();
    for cited in CITED.captures_iter(text) {
        let Some(list) = cited.get(1) else {
            continue;
        };
        let (quote, bounds) = evidence(text, list.range());
        let named = EXCHANGE
            .find(&text[bounds])
            .or_else(|| EXCHANGE.find(text))
            .map_or("other", |found| exchange(found.as_str()));
        for rule in RULE_NUMBER.find_iter(list.as_str()) {
            let rule = rule.as_str().to_owned();
            if rules.contains(&rule) {
                continue;
            }
            facts.push(json!({
                "fields": {"exchange": named, "rule": rule},
                "evidence": quote,
            }));
            rules.push(rule);
        }
    }
    if facts.is_empty()
        && let Some(found) = EXCHANGE.find(text)
    {
        facts.push(json!({
            "fields": {"exchange": exchange(found.as_str()), "rule": null},
            "evidence": evidence(text, found.range()).0,
        }));
    }
    json!({ "facts": facts })
}
