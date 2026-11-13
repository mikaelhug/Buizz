use std::collections::BTreeMap;
use std::fmt::Write;

use buizz_ingest::recipe::{RECIPES, Recipe};
use serde_json::Value;

fn class_name(recipe: &str) -> String {
    recipe
        .split('_')
        .map(|word| {
            let mut letters = word.chars();
            letters.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + letters.as_str()
            })
        })
        .collect()
}

fn kinds(property: &Value) -> Vec<&str> {
    match property.get("type") {
        Some(Value::String(kind)) => vec![kind.as_str()],
        Some(Value::Array(kinds)) => kinds.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

fn python_type(property: &Value) -> String {
    if let Some(Value::Array(values)) = property.get("enum") {
        let names: Vec<String> = values
            .iter()
            .filter_map(Value::as_str)
            .map(|value| format!("{value:?}"))
            .collect();
        return format!("Literal[{}]", names.join(", "));
    }
    let kinds = kinds(property);
    let kind = kinds.iter().find(|kind| **kind != "null").copied();
    match kind {
        Some("string") if property.get("format").and_then(Value::as_str) == Some("date") => {
            "date".to_owned()
        }
        Some("string") => "str".to_owned(),
        Some("integer") => "int".to_owned(),
        Some("number") => "Decimal".to_owned(),
        Some("boolean") => "bool".to_owned(),
        Some("array") => {
            let items = property.get("items").map_or("Any".to_owned(), python_type);
            format!("list[{items}]")
        }
        _ => "dict[str, Any]".to_owned(),
    }
}

fn newest() -> Vec<&'static Recipe> {
    let mut newest: BTreeMap<&str, &Recipe> = BTreeMap::new();
    for recipe in &RECIPES {
        let kept = newest.entry(recipe.name).or_insert(recipe);
        if recipe.version > kept.version {
            *kept = recipe;
        }
    }
    newest.into_values().collect()
}

fn main() {
    let recipes = newest();
    let mut out = String::from("type FactStatus = Literal[\"ok\", \"not_stated\"]\n");
    for recipe in &recipes {
        let schema: Value = serde_json::from_str(recipe.schema).expect("a recipe schema is JSON");
        let properties = schema["properties"]
            .as_object()
            .expect("a recipe schema has properties");
        write!(
            out,
            "\n\nclass {}(msgspec.Struct, frozen=True, kw_only=True):\n    \
             symbol: str\n    accession: str\n    status: FactStatus\n    \
             available_at: datetime\n    evidence: str | None\n",
            class_name(recipe.name),
        )
        .expect("writing to a string");
        for (name, property) in properties {
            writeln!(out, "    {name}: {} | None = None", python_type(property))
                .expect("writing to a string");
        }
    }
    let classes: Vec<String> = recipes
        .iter()
        .map(|recipe| class_name(recipe.name))
        .collect();
    let names: Vec<String> = recipes
        .iter()
        .map(|recipe| format!("{:?}", recipe.name))
        .collect();
    write!(
        out,
        "\n\ntype Fact = {}\ntype RecipeName = Literal[{}]\n\n\
         RECIPES: dict[str, type[Fact]] = {{{}}}\n\n\n\
         class FactQueries:\n",
        classes.join(" | "),
        names.join(", "),
        recipes
            .iter()
            .map(|recipe| format!("{:?}: {}", recipe.name, class_name(recipe.name)))
            .collect::<Vec<_>>()
            .join(", "),
    )
    .expect("writing to a string");
    if recipes.len() > 1 {
        for recipe in &recipes {
            write!(
                out,
                "    @overload\n    def facts(self, recipe: Literal[{:?}], symbols: list[str], since: date) -> list[{}]: ...\n",
                recipe.name,
                class_name(recipe.name)
            )
            .expect("writing to a string");
        }
    }
    out.push_str(
        "    def facts(self, recipe: RecipeName, symbols: list[str], since: date) -> list[Fact]:\n        \
         return self._facts(recipe, symbols, since)\n\n    \
         def _facts(self, recipe: str, symbols: list[str], since: date) -> list[Fact]:\n        \
         raise NotImplementedError\n",
    );
    let typing: Vec<&str> = [
        ("Any", "Any"),
        ("Literal", "Literal["),
        ("overload", "@overload"),
    ]
    .into_iter()
    .filter(|(_, used)| out.contains(used))
    .map(|(name, _)| name)
    .collect();
    let decimal = if out.contains("Decimal") {
        "from decimal import Decimal\n"
    } else {
        ""
    };
    print!(
        "from __future__ import annotations\n\n\
         from datetime import date, datetime\n{decimal}\
         from typing import {}\n\n\
         import msgspec\n\n{out}",
        typing.join(", ")
    );
}
