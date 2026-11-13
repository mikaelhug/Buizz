mod accept;
mod job;
mod listing_notice;
mod numbers;

use buizz_core::market::{Dataset, RecipeScope, RecipeState, RecipeVersion};
use chrono::NaiveDate;
use serde_json::{Value, json};

pub use accept::{Answer, accept};

pub const EVIDENCE_CHARS: u64 = 400;
const ITEMS_START: NaiveDate = NaiveDate::from_ymd_opt(2004, 8, 23).expect("a valid date");

pub enum Producer {
    Code(fn(&str) -> Value),
    Model {
        model: &'static str,
        prompt: &'static str,
    },
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
}

pub const RECIPES: [Recipe; 1] = [Recipe {
    name: "listing_notice",
    version: 1,
    dataset: Dataset::ListingNotices,
    schema: include_str!("../../recipes/listing_notice/1.schema.json"),
    producer: Producer::Code(listing_notice::read),
    scope: Scope {
        items: &["3.01"],
        exhibits: &[],
        from: ITEMS_START,
    },
}];

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
            Producer::Model { model, prompt } => (
                model.to_owned(),
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
