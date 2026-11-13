use std::str::FromStr;

use buizz_core::market::{Fact, FactStatus, RecipeSection};
use buizz_core::time::Timestamp;
use chrono::{Months, NaiveDate};
use rust_decimal::Decimal;
use serde_json::{Map, Number, Value};

use super::Version;
use super::numbers::{declared_scales, is_written, written_values};

const MAX_LISTED_ERRORS: usize = 3;
const DATE_WINDOW: Months = Months::new(24);

pub enum Answer<'a> {
    Given(&'a str),
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
            collect_numbers(fields, &mut numbers);
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

fn collect_numbers<'a>(value: &'a Value, numbers: &mut Vec<&'a Number>) {
    match value {
        Value::Number(number) => numbers.push(number),
        Value::Array(items) => items.iter().for_each(|item| collect_numbers(item, numbers)),
        Value::Object(fields) => fields
            .values()
            .for_each(|field| collect_numbers(field, numbers)),
        _ => {}
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

fn collect_dates<'a>(schema: &Value, value: &'a Value, dates: &mut Vec<&'a str>) {
    match value {
        Value::String(text) if schema.get("format").and_then(Value::as_str) == Some("date") => {
            dates.push(text);
        }
        Value::Array(items) => {
            if let Some(item_schema) = schema.get("items") {
                items
                    .iter()
                    .for_each(|item| collect_dates(item_schema, item, dates));
            }
        }
        Value::Object(fields) => {
            let empty = Map::new();
            let properties = schema
                .get("properties")
                .and_then(Value::as_object)
                .unwrap_or(&empty);
            for (key, field) in fields {
                if let Some(property) = properties.get(key) {
                    collect_dates(property, field, dates);
                }
            }
        }
        _ => {}
    }
}
