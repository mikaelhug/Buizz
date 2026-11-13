use buizz_core::market::{DataSource, Dataset, Fact, RecipeScope, RecipeState, RecipeVersion};
use buizz_core::time::Timestamp;
use duckdb::{Connection, Transaction, params};
use serde_json::{Map, Value};

use crate::codec::{Ts, Wire, text_list, wire_name};
use crate::write::{Coverage, cover, write_batch};
use crate::{AccessionProgress, MarketError, Result, WriteSummary};

const LATEST: &str = "SELECT recipe, version, schema, producer, prompt_hash, scope, state, gold \
     FROM recipe_version QUALIFY row_number() OVER (PARTITION BY recipe, version \
     ORDER BY batch_id DESC) = 1";

const SHARED_COLUMNS: [&str; 13] = [
    "recipe",
    "version",
    "doc_id",
    "section_key",
    "ordinal",
    "accession",
    "cik",
    "status",
    "evidence",
    "evidence_start",
    "reason",
    "published_at",
    "produced_at",
];

const IN_SCOPE: &str = "WITH main_document AS (SELECT d.doc_id, d.content_hash, d.accession, \
     d.cik, d.published_at, f.volume, f.path FROM text_document d \
     JOIN text_file f ON f.id = d.file_id WHERE d.kind IN ('8-K', '8-K/A') AND d.published_at >= ? \
     QUALIFY row_number() OVER (PARTITION BY d.doc_id ORDER BY d.batch_id DESC) = 1), \
     reported AS (SELECT DISTINCT accession, unnest(items) AS item FROM current_report), \
     in_scope AS (SELECT m.*, '8-K:' || r.item AS section_key FROM main_document m \
     JOIN reported r USING (accession) WHERE r.item IN (SELECT unnest(?::VARCHAR[])))";

pub(crate) struct SectionLocation {
    pub(crate) doc_id: String,
    pub(crate) content_hash: String,
    pub(crate) section_key: String,
    pub(crate) accession: Option<String>,
    pub(crate) cik: Option<u32>,
    pub(crate) published_at: Timestamp,
    pub(crate) volume: String,
    pub(crate) path: String,
}

fn identifier(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn scope_json(scope: &RecipeScope) -> String {
    serde_json::to_string(scope).expect("a recipe scope serialises")
}

fn latest(
    connection: &Connection,
    filter: &str,
    values: &[&dyn duckdb::ToSql],
) -> Result<Vec<RecipeVersion>> {
    let mut statement = connection.prepare(&format!(
        "SELECT * FROM ({LATEST}) WHERE {filter} ORDER BY recipe, version"
    ))?;
    let rows = statement
        .query_map(values, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Wire<RecipeState>>(6)?.0,
                row.get::<_, Option<String>>(7)?,
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(
            |(recipe, version, schema, producer, prompt_hash, scope, state, gold)| {
                let scope = serde_json::from_str(&scope).map_err(|error| {
                    MarketError::Recipe(format!(
                        "{recipe} version {version} has an unreadable scope: {error}"
                    ))
                })?;
                Ok(RecipeVersion {
                    recipe,
                    version,
                    schema,
                    producer,
                    prompt_hash,
                    scope,
                    state,
                    gold,
                })
            },
        )
        .collect()
}

pub(crate) fn recipe_versions(connection: &Connection) -> Result<Vec<RecipeVersion>> {
    latest(connection, "true", &[])
}

fn stored(connection: &Connection, recipe: &str, version: u32) -> Result<Option<RecipeVersion>> {
    Ok(latest(
        connection,
        "recipe = ? AND version = ?",
        &[&recipe, &version],
    )?
    .into_iter()
    .next())
}

fn append(transaction: &Transaction<'_>, version: &RecipeVersion, batch_id: i64) -> Result<u64> {
    Ok(transaction.execute(
        "INSERT INTO main.recipe_version VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            version.recipe,
            version.version,
            version.schema,
            version.producer,
            version.prompt_hash,
            scope_json(&version.scope),
            wire_name(&version.state),
            version.gold,
            batch_id,
        ],
    )? as u64)
}

pub(crate) fn register(
    connection: &mut Connection,
    source: DataSource,
    dataset: Dataset,
    version: RecipeVersion,
) -> Result<RecipeVersion> {
    if !identifier(&version.recipe) {
        return Err(MarketError::Recipe(format!(
            "{:?} is not a recipe name: use lower case letters, digits and underscores",
            version.recipe
        )));
    }
    typed_columns(&version.schema)
        .map_err(|reason| MarketError::Recipe(format!("{}: {reason}", version.recipe)))?;
    if let Some(known) = stored(connection, &version.recipe, version.version)? {
        if (
            &known.schema,
            &known.producer,
            &known.prompt_hash,
            &known.scope,
        ) != (
            &version.schema,
            &version.producer,
            &version.prompt_hash,
            &version.scope,
        ) {
            return Err(MarketError::Recipe(format!(
                "{} version {} differs from the stored version; a version is immutable, so the \
                 change needs a new version",
                version.recipe, version.version
            )));
        }
        sync_views(connection)?;
        return Ok(known);
    }
    write_batch(
        connection,
        source,
        dataset,
        Coverage::new(),
        |transaction, batch_id| append(transaction, &version, batch_id),
    )?;
    sync_views(connection)?;
    Ok(version)
}

pub(crate) fn advance(
    connection: &mut Connection,
    source: DataSource,
    dataset: Dataset,
    recipe: &str,
    version: u32,
    to: RecipeState,
    gold: Option<String>,
) -> Result<()> {
    let refuse =
        |reason: String| MarketError::Recipe(format!("{recipe} version {version}: {reason}"));
    let known = stored(connection, recipe, version)?
        .ok_or_else(|| refuse("is not registered".to_owned()))?;
    let gold = gold.or(known.gold.clone());
    match (known.state, to) {
        (RecipeState::Evaluating, RecipeState::Filling)
            if gold.is_none() && known.producer != "code" =>
        {
            return Err(refuse(
                "needs a passing gold report before it fills".to_owned(),
            ));
        }
        (RecipeState::Evaluating, RecipeState::Filling)
        | (RecipeState::Complete, RecipeState::Current) => {}
        (RecipeState::Filling, RecipeState::Complete) => {
            let progress = recipe_progress(connection, recipe, version, &known.scope)?;
            if progress.done + progress.failed < progress.expected {
                return Err(refuse(format!(
                    "{} of {} sections have no row yet",
                    progress.expected - progress.done - progress.failed,
                    progress.expected
                )));
            }
        }
        (from, RecipeState::Retired) if from != RecipeState::Retired => {}
        (from, to) => {
            return Err(refuse(format!(
                "cannot move from {} to {}",
                wire_name(&from),
                wire_name(&to)
            )));
        }
    }
    let replaced = if to == RecipeState::Current {
        latest(connection, "recipe = ? AND state = 'current'", &[&recipe])?
    } else {
        Vec::new()
    };
    write_batch(
        connection,
        source,
        dataset,
        Coverage::new(),
        |transaction, batch_id| {
            let mut rows = append(
                transaction,
                &RecipeVersion {
                    state: to,
                    gold,
                    ..known
                },
                batch_id,
            )?;
            for old in replaced {
                rows += append(
                    transaction,
                    &RecipeVersion {
                        state: RecipeState::Retired,
                        ..old
                    },
                    batch_id,
                )?;
            }
            Ok(rows)
        },
    )?;
    sync_views(connection)
}

pub(crate) fn typed_columns(schema: &str) -> std::result::Result<Vec<(String, String)>, String> {
    let schema: Value =
        serde_json::from_str(schema).map_err(|error| format!("the schema is not JSON: {error}"))?;
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or("the schema of fields must be an object with properties")?;
    properties
        .iter()
        .map(|(name, property)| {
            if !identifier(name) || SHARED_COLUMNS.contains(&name.as_str()) {
                return Err(format!(
                    "field {name:?} cannot be a column: use lower case letters, digits and \
                     underscores, and none of the shared column names"
                ));
            }
            let kind = match property.get("type") {
                Some(Value::String(kind)) => Some(kind.as_str()),
                Some(Value::Array(kinds)) => kinds
                    .iter()
                    .filter_map(Value::as_str)
                    .find(|kind| *kind != "null"),
                _ => None,
            };
            let date = property.get("format").and_then(Value::as_str) == Some("date");
            let sql = match (kind, date) {
                (Some("string"), true) => "DATE",
                (Some("string"), false) => "VARCHAR",
                (None, _) if property.get("enum").is_some() => "VARCHAR",
                (Some("integer"), _) => "BIGINT",
                (Some("number"), _) => "DECIMAL(38, 10)",
                (Some("boolean"), _) => "BOOLEAN",
                (Some("array" | "object"), _) => {
                    return Ok((
                        name.clone(),
                        format!("json_extract(fields, '$.{name}') AS {name}"),
                    ));
                }
                _ => return Err(format!("field {name:?} has no column type")),
            };
            Ok((
                name.clone(),
                format!("CAST(json_extract_string(fields, '$.{name}') AS {sql}) AS {name}"),
            ))
        })
        .collect()
}

fn sync_views(connection: &Connection) -> Result<()> {
    let current = latest(connection, "state = 'current'", &[])?;
    let mut statement = connection.prepare(
        "SELECT view_name FROM duckdb_views() WHERE schema_name = 'main' \
         AND view_name LIKE 'facts\\_%' ESCAPE '\\'",
    )?;
    let existing: Vec<String> = statement
        .query_map([], |row| row.get(0))?
        .collect::<duckdb::Result<_>>()?;
    for stale in existing.iter().filter(|view| {
        !current
            .iter()
            .any(|version| **view == format!("facts_{}", version.recipe))
    }) {
        connection.execute_batch(&format!("DROP VIEW main.{stale}"))?;
    }
    for version in current {
        let columns: Vec<String> = typed_columns(&version.schema)
            .map_err(|reason| MarketError::Recipe(format!("{}: {reason}", version.recipe)))?
            .into_iter()
            .map(|(_, column)| column)
            .collect();
        connection.execute_batch(&format!(
            "CREATE OR REPLACE VIEW main.facts_{recipe} AS SELECT {shared}, {typed} FROM main.fact \
             WHERE recipe = '{recipe}' AND version = {number}",
            recipe = version.recipe,
            number = version.version,
            shared = SHARED_COLUMNS.join(", "),
            typed = columns.join(", "),
        ))?;
    }
    Ok(())
}

pub(crate) fn unread_sections(
    connection: &Connection,
    recipe: &str,
    version: u32,
    scope: &RecipeScope,
    limit: usize,
) -> Result<Vec<SectionLocation>> {
    let mut statement = connection.prepare_cached(&format!(
        "{IN_SCOPE} SELECT doc_id, content_hash, section_key, accession, cik, published_at, volume, \
         path FROM in_scope s WHERE NOT EXISTS (SELECT 1 FROM fact x WHERE x.recipe = ? \
         AND x.version = ? AND x.doc_id = s.doc_id AND x.section_key = s.section_key) \
         ORDER BY published_at DESC, doc_id, section_key LIMIT ?"
    ))?;
    let sections = statement
        .query_map(
            params![
                scope.from,
                text_list(scope.items.iter().map(String::as_str)),
                recipe,
                version,
                limit as u64
            ],
            |row| {
                Ok(SectionLocation {
                    doc_id: row.get(0)?,
                    content_hash: row.get(1)?,
                    section_key: row.get(2)?,
                    accession: row.get(3)?,
                    cik: row.get(4)?,
                    published_at: row.get::<_, Ts>(5)?.0,
                    volume: row.get(6)?,
                    path: row.get(7)?,
                })
            },
        )?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(sections)
}

pub(crate) fn recipe_progress(
    connection: &Connection,
    recipe: &str,
    version: u32,
    scope: &RecipeScope,
) -> Result<AccessionProgress> {
    Ok(connection.query_row(
        &format!(
            "{IN_SCOPE} SELECT count(*), count(*) FILTER (WHERE answered AND NOT failed), \
             count(*) FILTER (WHERE failed) FROM (SELECT \
             EXISTS (SELECT 1 FROM fact x WHERE x.recipe = ? AND x.version = ? \
             AND x.doc_id = s.doc_id AND x.section_key = s.section_key) AS answered, \
             EXISTS (SELECT 1 FROM fact x WHERE x.recipe = ? AND x.version = ? \
             AND x.doc_id = s.doc_id AND x.section_key = s.section_key \
             AND x.status = 'failed') AS failed FROM in_scope s)"
        ),
        params![
            scope.from,
            text_list(scope.items.iter().map(String::as_str)),
            recipe,
            version,
            recipe,
            version
        ],
        |row| {
            Ok(AccessionProgress {
                expected: row.get(0)?,
                done: row.get(1)?,
                failed: row.get(2)?,
            })
        },
    )?)
}

pub(crate) fn facts(
    connection: &mut Connection,
    source: DataSource,
    dataset: Dataset,
    facts: Vec<Fact>,
) -> Result<WriteSummary> {
    let mut coverage = Coverage::new();
    for fact in &facts {
        cover(&mut coverage, None, fact.published_at.exchange_date());
    }
    write_batch(
        connection,
        source,
        dataset,
        coverage,
        |transaction, batch_id| {
            let mut statement = transaction.prepare_cached(
            "INSERT OR IGNORE INTO main.fact VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )?;
            let mut rows = 0;
            for fact in &facts {
                rows += statement.execute(params![
                    fact.recipe,
                    fact.version,
                    fact.doc_id,
                    fact.section_key,
                    fact.ordinal,
                    fact.accession,
                    fact.cik,
                    wire_name(&fact.status),
                    fact.fields,
                    fact.evidence,
                    fact.evidence_start,
                    fact.reason,
                    Ts(fact.published_at),
                    Ts(fact.produced_at),
                    fact.input_hash,
                    batch_id,
                ])? as u64;
            }
            Ok(rows)
        },
    )
}

pub(crate) fn document_facts(
    connection: &Connection,
    doc_id: &str,
) -> Result<Vec<Map<String, Value>>> {
    let mut rows = Vec::new();
    for version in latest(connection, "state = 'current'", &[])? {
        let mut statement = connection.prepare(&format!(
            "SELECT to_json(v)::VARCHAR FROM main.facts_{} v WHERE doc_id = ? \
             ORDER BY section_key, ordinal",
            version.recipe
        ))?;
        let found = statement
            .query_map([doc_id], |row| row.get::<_, String>(0))?
            .collect::<duckdb::Result<Vec<_>>>()?;
        for json in found {
            rows.push(serde_json::from_str(&json).map_err(|error| {
                MarketError::Recipe(format!("facts_{} row: {error}", version.recipe))
            })?);
        }
    }
    Ok(rows)
}
