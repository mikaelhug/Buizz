use std::str::FromStr;

use buizz_core::market::{Fact, FactStatus, RecipeSection};
use buizz_core::time::Timestamp;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{Gold, Version};

include!(concat!(env!("OUT_DIR"), "/gold.rs"));

const ROWS_FOUND: Decimal = Decimal::from_parts(97, 0, 0, false, 0);
const ROWS_REAL: Decimal = Decimal::from_parts(98, 0, 0, false, 0);
const STATED_RIGHT: Decimal = Decimal::from_parts(98, 0, 0, false, 0);
const ENUMS_EXACT: Decimal = Decimal::from_parts(97, 0, 0, false, 0);
const VALUES_EXACT: Decimal = Decimal::from_parts(99, 0, 0, false, 0);
const NAMES_RIGHT: Decimal = Decimal::from_parts(95, 0, 0, false, 0);
const HUNDRED: Decimal = Decimal::from_parts(100, 0, 0, false, 0);

pub(crate) struct GoldFile {
    recipe: &'static str,
    split: &'static str,
    text: &'static str,
    label: &'static str,
}

#[derive(Deserialize)]
struct Label {
    accession: String,
    cik: u32,
    published_at: Timestamp,
    facts: Vec<LabelledFact>,
}

#[derive(Deserialize)]
struct LabelledFact {
    fields: Value,
}

pub(crate) struct GoldDocument {
    pub(crate) section: RecipeSection,
    facts: Vec<Value>,
}

pub(crate) fn documents(recipe: &str, split: &str) -> Result<Vec<GoldDocument>, String> {
    GOLD_FILES
        .iter()
        .filter(|file| file.recipe == recipe && file.split == split)
        .map(|file| {
            let label: Label = serde_json::from_str(file.label)
                .map_err(|error| format!("a {recipe} {split} gold label: {error}"))?;
            Ok(GoldDocument {
                section: RecipeSection {
                    doc_id: format!("gold:{}", label.accession),
                    section_key: String::new(),
                    accession: Some(label.accession),
                    cik: Some(label.cik),
                    published_at: label.published_at,
                    text: Ok(file.text.to_owned()),
                },
                facts: label.facts.into_iter().map(|fact| fact.fields).collect(),
            })
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Measure {
    pub measure: String,
    pub hits: u64,
    pub total: u64,
    pub target: Decimal,
    pub passed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoldReport {
    pub recipe: String,
    pub version: u32,
    pub producer: String,
    pub prompt_hash: Option<String>,
    pub split: String,
    pub documents: usize,
    pub failed_rows: u64,
    pub usd: Decimal,
    pub passed: bool,
    pub measures: Vec<Measure>,
}

impl Measure {
    fn new(measure: impl Into<String>, hits: u64, total: u64, target: Decimal) -> Self {
        Self {
            measure: measure.into(),
            hits,
            total,
            target,
            passed: total == 0 || Decimal::from(hits) * HUNDRED >= target * Decimal::from(total),
        }
    }
}

pub(crate) fn same_number(gold: &Value, answer: &Value) -> bool {
    match (gold, answer) {
        (Value::Number(gold), Value::Number(answer)) => {
            let read = |number: &serde_json::Number| {
                let text = number.to_string();
                Decimal::from_str(&text)
                    .or_else(|_| Decimal::from_scientific(&text))
                    .ok()
            };
            read(gold).is_some_and(|gold| Some(gold) == read(answer))
        }
        (gold, answer) => gold == answer,
    }
}

fn normalised(name: &Value) -> String {
    name.as_str()
        .unwrap_or_default()
        .to_lowercase()
        .chars()
        .filter(|character| character.is_alphanumeric() || character.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

enum Kind {
    Enum,
    Value,
    Name,
}

fn kinds(version: &Version, gold: &Gold) -> Vec<(String, Kind)> {
    let empty = Map::new();
    let properties = version.fields["properties"].as_object().unwrap_or(&empty);
    properties
        .iter()
        .filter_map(|(name, property)| {
            let typed = |kind: &str| match &property["type"] {
                Value::String(named) => named == kind,
                Value::Array(named) => named.iter().any(|named| named == kind),
                _ => false,
            };
            let kind = if gold.names.contains(&name.as_str()) {
                Kind::Name
            } else if property.get("enum").is_some() {
                Kind::Enum
            } else if typed("number")
                || typed("integer")
                || typed("boolean")
                || property["format"] == "date"
            {
                Kind::Value
            } else {
                return None;
            };
            Some((name.clone(), kind))
        })
        .collect()
}

fn equal(kind: &Kind, gold: &Value, answer: &Value) -> bool {
    match kind {
        Kind::Name => normalised(gold) == normalised(answer),
        Kind::Enum | Kind::Value => same_number(gold, answer),
    }
}

pub(crate) fn score(
    version: &Version,
    documents: &[GoldDocument],
    answers: &[Vec<Fact>],
    split: &str,
) -> GoldReport {
    let gold = version
        .recipe
        .gold
        .as_ref()
        .expect("a recipe with a gold set");
    let fields = kinds(version, gold);
    let (mut found, mut labelled, mut given, mut stated_right, mut failed_rows) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut field_hits = vec![(0u64, 0u64); fields.len()];
    for (document, rows) in documents.iter().zip(answers) {
        let ok: Vec<Value> = rows
            .iter()
            .filter(|row| row.status == FactStatus::Ok)
            .filter_map(|row| row.fields.as_deref())
            .filter_map(|fields| serde_json::from_str(fields).ok())
            .collect();
        failed_rows += rows
            .iter()
            .filter(|row| row.status == FactStatus::Failed)
            .count() as u64;
        let not_stated = rows.len() == 1 && rows[0].status == FactStatus::NotStated;
        stated_right += u64::from(if document.facts.is_empty() {
            not_stated
        } else {
            !ok.is_empty()
        });
        labelled += document.facts.len() as u64;
        given += ok.len() as u64;
        let mut pairs: Vec<(usize, usize, usize)> = Vec::new();
        for (g, expected) in document.facts.iter().enumerate() {
            for (a, answered) in ok.iter().enumerate() {
                if (gold.same_fact)(expected, answered) {
                    let agreeing = fields
                        .iter()
                        .filter(|(name, kind)| equal(kind, &expected[name], &answered[name]))
                        .count();
                    pairs.push((agreeing, g, a));
                }
            }
        }
        pairs.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
        let (mut gold_used, mut answer_used) =
            (vec![false; document.facts.len()], vec![false; ok.len()]);
        for (_, g, a) in pairs {
            if gold_used[g] || answer_used[a] {
                continue;
            }
            gold_used[g] = true;
            answer_used[a] = true;
            found += 1;
            for ((name, kind), hits) in fields.iter().zip(field_hits.iter_mut()) {
                hits.1 += 1;
                hits.0 += u64::from(equal(kind, &document.facts[g][name], &ok[a][name]));
            }
        }
    }
    let mut measures = vec![
        Measure::new("rows found", found, labelled, ROWS_FOUND),
        Measure::new("rows real", found, given, ROWS_REAL),
        Measure::new(
            "ok or not_stated",
            stated_right,
            documents.len() as u64,
            STATED_RIGHT,
        ),
    ];
    for ((name, kind), (hits, total)) in fields.iter().zip(field_hits) {
        let target = match kind {
            Kind::Enum => ENUMS_EXACT,
            Kind::Value => VALUES_EXACT,
            Kind::Name => NAMES_RIGHT,
        };
        measures.push(Measure::new(name.clone(), hits, total, target));
    }
    let stored = version.stored();
    GoldReport {
        recipe: stored.recipe,
        version: stored.version,
        producer: stored.producer,
        prompt_hash: stored.prompt_hash,
        split: split.to_owned(),
        documents: documents.len(),
        failed_rows,
        usd: Decimal::ZERO,
        passed: !documents.is_empty() && measures.iter().all(|measure| measure.passed),
        measures,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Answer, RECIPES, Version, accept};
    use super::*;

    #[test]
    fn every_gold_label_passes_the_checks_it_scores_a_model_by() {
        let mut refused = Vec::new();
        for recipe in RECIPES.iter().filter(|recipe| recipe.gold.is_some()) {
            let version = Version::new(recipe).unwrap();
            for split in ["dev", "test"] {
                let documents = documents(recipe.name, split).unwrap();
                assert!(!documents.is_empty(), "{} has a {split} split", recipe.name);
                let answers: Vec<Vec<Fact>> = GOLD_FILES
                    .iter()
                    .filter(|file| file.recipe == recipe.name && file.split == split)
                    .zip(&documents)
                    .map(|(file, document)| {
                        let label: Value = serde_json::from_str(file.label).unwrap();
                        let answer = serde_json::json!({"facts": label["facts"]}).to_string();
                        let rows = accept(
                            &version,
                            &document.section,
                            file.text,
                            Answer::Given(&answer),
                        );
                        let reasons: Vec<&str> = rows
                            .iter()
                            .filter_map(|row| row.reason.as_deref())
                            .collect();
                        if !reasons.is_empty() {
                            refused.push(format!(
                                "{} {split} {}: {reasons:?}",
                                recipe.name, label["accession"]
                            ));
                        }
                        rows
                    })
                    .collect();
                let report = score(&version, &documents, &answers, split);
                assert!(
                    !refused.is_empty() || report.passed,
                    "the labels score themselves perfectly: {report:#?}"
                );
            }
        }
        assert!(refused.is_empty(), "{refused:#?}");
    }
}
