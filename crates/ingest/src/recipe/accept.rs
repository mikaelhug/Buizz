use std::str::FromStr;

use buizz_core::market::{Fact, FactStatus, RecipeSection};
use buizz_core::time::Timestamp;
use chrono::{Months, NaiveDate};
use rust_decimal::Decimal;
use serde_json::{Number, Value};

use super::Version;
use super::numbers::{declared_scales, is_written, written_values};

const MAX_LISTED_ERRORS: usize = 3;
const DATE_WINDOW: Months = Months::new(24);

pub enum Answer<'a> {
    Given(&'a str),
    Failed(String),
    Skipped(String),
}

struct Row {
    status: FactStatus,
    fields: Option<String>,
    evidence: Option<String>,
    evidence_start: Option<u64>,
    reason: Option<String>,
}

impl Row {
    fn failed(evidence: Option<&str>, reason: String) -> Self {
        Self {
            status: FactStatus::Failed,
            fields: None,
            evidence: evidence.map(str::to_owned),
            evidence_start: None,
            reason: Some(reason),
        }
    }

    fn bare(status: FactStatus, reason: Option<String>) -> Self {
        Self {
            status,
            fields: None,
            evidence: None,
            evidence_start: None,
            reason,
        }
    }
}

pub fn accept(
    version: &Version,
    doc: &RecipeSection,
    section_text: &str,
    answer: Answer<'_>,
) -> Vec<Fact> {
    let produced_at = Timestamp::now();
    let input_hash = blake3::hash(section_text.as_bytes()).to_hex().to_string();
    let rows = match answer {
        Answer::Skipped(reason) => vec![Row::bare(FactStatus::Skipped, Some(reason))],
        Answer::Failed(reason) => vec![Row::failed(None, reason)],
        Answer::Given(raw) => checked(version, doc, section_text, raw),
    };
    rows.into_iter()
        .enumerate()
        .map(|(ordinal, row)| Fact {
            recipe: version.recipe.name.to_owned(),
            version: version.recipe.version,
            doc_id: doc.doc_id.clone(),
            section_key: doc.section_key.clone(),
            ordinal: u32::try_from(ordinal).unwrap_or(u32::MAX),
            accession: doc.accession.clone(),
            cik: doc.cik,
            status: row.status,
            fields: row.fields,
            evidence: row.evidence,
            evidence_start: row.evidence_start,
            reason: row.reason,
            published_at: doc.published_at,
            produced_at,
            input_hash: input_hash.clone(),
        })
        .collect()
}

fn checked(version: &Version, doc: &RecipeSection, section_text: &str, raw: &str) -> Vec<Row> {
    let answer: Value = match serde_json::from_str(raw) {
        Ok(answer) => answer,
        Err(error) => {
            return vec![Row::failed(
                None,
                format!("the answer is not JSON: {error}"),
            )];
        }
    };
    let errors: Vec<String> = version
        .answer
        .iter_errors(&answer)
        .take(MAX_LISTED_ERRORS)
        .map(|error| format!("{} at {}", error, error.instance_path()))
        .collect();
    if !errors.is_empty() {
        return vec![Row::failed(
            None,
            format!(
                "the answer does not match the schema: {}",
                errors.join("; ")
            ),
        )];
    }
    let facts = answer["facts"].as_array().map_or(&[][..], Vec::as_slice);
    if facts.is_empty() {
        return vec![Row::bare(FactStatus::NotStated, None)];
    }
    let section = Normalised::new(section_text);
    let declared = declared_scales(section_text);
    let filed = doc.published_at.exchange_date();
    facts
        .iter()
        .map(|fact| {
            let fields = &fact["fields"];
            let evidence = fact["evidence"].as_str().unwrap_or_default();
            let mut reasons = Vec::new();
            let start = section.find(evidence);
            if start.is_none() {
                reasons.push("the evidence does not occur in the section".to_owned());
            }
            let written = written_values(evidence, &declared);
            let mut numbers = Vec::new();
            collect_numbers(&version.fields, fields, &mut numbers);
            for number in numbers {
                match decimal(number) {
                    Some(value) if is_written(value, &written) => {}
                    _ => reasons.push(format!("{number} is not written in the evidence")),
                }
            }
            check_ranges(fields, &mut reasons);
            let mut dates = Vec::new();
            collect_dates(&version.fields, fields, &mut dates);
            for date in dates {
                match NaiveDate::from_str(date) {
                    Ok(date)
                        if filed
                            .checked_sub_months(DATE_WINDOW)
                            .is_some_and(|from| date >= from)
                            && filed
                                .checked_add_months(DATE_WINDOW)
                                .is_some_and(|to| date <= to) => {}
                    _ => reasons.push(format!("{date} is not within two years of the filing")),
                }
            }
            match (start, reasons.is_empty()) {
                (Some(start), true) => Row {
                    status: FactStatus::Ok,
                    fields: Some(fields.to_string()),
                    evidence: Some(evidence.to_owned()),
                    evidence_start: Some(start),
                    reason: None,
                },
                _ => Row::failed(Some(evidence), reasons.join("; ")),
            }
        })
        .collect()
}

struct Normalised {
    text: String,
    origins: Vec<u64>,
}

impl Normalised {
    fn new(source: &str) -> Self {
        let mut text = String::with_capacity(source.len());
        let mut origins = Vec::new();
        let mut pending_space = None;
        for (index, character) in source.chars().enumerate() {
            let index = index as u64;
            if character.is_whitespace() {
                pending_space = pending_space.or(Some(index));
                continue;
            }
            if let Some(space) = pending_space.take()
                && !text.is_empty()
            {
                text.push(' ');
                origins.push(space);
            }
            text.push(character);
            origins.push(index);
        }
        Self { text, origins }
    }

    fn find(&self, quote: &str) -> Option<u64> {
        let quote = Self::new(quote).text;
        if quote.is_empty() {
            return None;
        }
        let byte = self.text.find(&quote)?;
        self.origins.get(self.text[..byte].chars().count()).copied()
    }
}

fn decimal(number: &Number) -> Option<Decimal> {
    let text = number.to_string();
    Decimal::from_str(&text)
        .or_else(|_| Decimal::from_scientific(&text))
        .ok()
}

fn check_ranges(value: &Value, reasons: &mut Vec<String>) {
    let Value::Object(fields) = value else {
        return;
    };
    for (key, low) in fields {
        let Some(prefix) = key.strip_suffix("low") else {
            if let Value::Object(_) = low {
                check_ranges(low, reasons);
            }
            continue;
        };
        if !(prefix.is_empty() || prefix.ends_with('_')) {
            continue;
        }
        let high_key = format!("{prefix}high");
        let bound = |field: Option<&Value>| field.and_then(Value::as_number).and_then(decimal);
        if let (Some(low_value), Some(high_value)) =
            (bound(Some(low)), bound(fields.get(&high_key)))
            && low_value > high_value
        {
            reasons.push(format!(
                "{key} {low_value} is above {high_key} {high_value}"
            ));
        }
    }
}

fn leaves<'a, 's>(schema: &'s Value, value: &'a Value, found: &mut Vec<(&'s Value, &'a Value)>) {
    match value {
        Value::Array(items) => {
            if let Some(item_schema) = schema.get("items") {
                items
                    .iter()
                    .for_each(|item| leaves(item_schema, item, found));
            }
        }
        Value::Object(fields) => {
            let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
                return;
            };
            for (key, field) in fields {
                if let Some(property) = properties.get(key) {
                    leaves(property, field, found);
                }
            }
        }
        _ => found.push((schema, value)),
    }
}

fn typed(schema: &Value, kind: &str) -> bool {
    match schema.get("type") {
        Some(Value::String(named)) => named == kind,
        Some(Value::Array(named)) => named.iter().any(|named| named == kind),
        _ => false,
    }
}

fn collect_numbers<'a>(schema: &Value, value: &'a Value, numbers: &mut Vec<&'a Number>) {
    let mut found = Vec::new();
    leaves(schema, value, &mut found);
    numbers.extend(found.into_iter().filter_map(|(schema, value)| match value {
        Value::Number(number) if typed(schema, "number") => Some(number),
        _ => None,
    }));
}

fn collect_dates<'a>(schema: &Value, value: &'a Value, dates: &mut Vec<&'a str>) {
    let mut found = Vec::new();
    leaves(schema, value, &mut found);
    dates.extend(found.into_iter().filter_map(|(schema, value)| match value {
        Value::String(text) if schema.get("format").and_then(Value::as_str) == Some("date") => {
            Some(text.as_str())
        }
        _ => None,
    }));
}
