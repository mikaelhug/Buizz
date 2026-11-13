mod accept;
mod earnings_guidance;
mod gold;
mod job;
mod listing_notice;
mod model;
mod numbers;

use buizz_core::market::{Dataset, RecipeScope, RecipeState, RecipeVersion};
use buizz_sources::anthropic::{MODEL, Prompt};
use chrono::NaiveDate;
use serde_json::{Value, json};

pub use accept::{Answer, accept};
pub use gold::{GoldReport, Measure};
pub use model::{Tally, ask, trial};

pub const EVIDENCE_CHARS: u64 = 400;
const ITEMS_START: NaiveDate = NaiveDate::from_ymd_opt(2004, 8, 23).expect("a valid date");

pub enum Producer {
    Code(fn(&str) -> Value),
    Model {
        prompt: &'static str,
        max_tokens: u32,
    },
}

pub struct Gold {
    pub same_fact: fn(&Value, &Value) -> bool,
    pub names: &'static [&'static str],
}

pub struct Scope {
    pub items: &'static [&'static str],
    pub exhibits: &'static [&'static str],
    pub from: NaiveDate,
}

pub struct Recipe {
    pub name: &'static str,
    pub version: u32,
    pub dataset: Dataset,
    pub schema: &'static str,
    pub producer: Producer,
    pub scope: Scope,
    pub gold: Option<Gold>,
}

pub const RECIPES: [Recipe; 2] = [
    Recipe {
        name: "listing_notice",
        version: 2,
        dataset: Dataset::ListingNotices,
        schema: include_str!("../../recipes/listing_notice/1.schema.json"),
        producer: Producer::Code(listing_notice::read),
        scope: Scope {
            items: &["3.01"],
            exhibits: &[],
            from: ITEMS_START,
        },
        gold: None,
    },
    Recipe {
        name: "earnings_guidance",
        version: 1,
        dataset: Dataset::EarningsGuidance,
        schema: include_str!("../../recipes/earnings_guidance/1.schema.json"),
        producer: Producer::Model {
            prompt: include_str!("../../recipes/earnings_guidance/1.prompt.md"),
            max_tokens: 8192,
        },
        scope: Scope {
            items: &["2.02"],
            exhibits: &["EX-99"],
            from: ITEMS_START,
        },
        gold: Some(Gold {
            same_fact: earnings_guidance::same_fact,
            names: &[],
        }),
    },
];

pub fn recipe(name: &str) -> Option<&'static Recipe> {
    RECIPES.iter().find(|recipe| recipe.name == name)
}

pub struct Version {
    pub recipe: &'static Recipe,
    fields: Value,
    answer: jsonschema::Validator,
}

impl Version {
    pub fn new(recipe: &'static Recipe) -> Result<Self, String> {
        let invalid = |error: &dyn std::fmt::Display| {
            format!("{} version {}: {error}", recipe.name, recipe.version)
        };
        let fields: Value = serde_json::from_str(recipe.schema).map_err(|error| invalid(&error))?;
        let answer = jsonschema::options()
            .should_validate_formats(true)
            .build(&answer_schema(&fields))
            .map_err(|error| invalid(&error))?;
        Ok(Self {
            recipe,
            fields,
            answer,
        })
    }

    pub fn stored(&self) -> RecipeVersion {
        let (producer, prompt_hash, state) = match self.recipe.producer {
            Producer::Code(_) => ("code".to_owned(), None, RecipeState::Filling),
            Producer::Model { prompt, .. } => (
                MODEL.to_owned(),
                Some(blake3::hash(prompt.as_bytes()).to_hex().to_string()),
                RecipeState::Evaluating,
            ),
        };
        RecipeVersion {
            recipe: self.recipe.name.to_owned(),
            version: self.recipe.version,
            schema: self.recipe.schema.to_owned(),
            producer,
            prompt_hash,
            scope: RecipeScope {
                items: self
                    .recipe
                    .scope
                    .items
                    .iter()
                    .map(|item| (*item).to_owned())
                    .collect(),
                exhibits: self
                    .recipe
                    .scope
                    .exhibits
                    .iter()
                    .map(|kind| (*kind).to_owned())
                    .collect(),
                from: self.recipe.scope.from,
            },
            state,
            gold: None,
        }
    }

    pub fn prompt(&self) -> Option<Prompt> {
        let Producer::Model { prompt, max_tokens } = self.recipe.producer else {
            return None;
        };
        Some(Prompt {
            system: prompt.to_owned(),
            schema: answer_schema(&self.fields),
            max_tokens,
        })
    }
}

fn answer_schema(fields: &Value) -> Value {
    json!({
        "type": "object",
        "properties": {
            "facts": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "fields": fields,
                        "evidence": {"type": "string", "maxLength": EVIDENCE_CHARS},
                    },
                    "required": ["fields", "evidence"],
                    "additionalProperties": false,
                },
            },
        },
        "required": ["facts"],
        "additionalProperties": false,
    })
}
