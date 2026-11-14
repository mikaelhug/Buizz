use std::str::FromStr;

use buizz_core::market::{
    Calendar, DataSource, Dataset, Fact, FactStatus, ModelBatch, ModelRequest, ModelSpend,
    RecipeScope, RecipeState, RecipeVersion, Symbol,
};
use buizz_core::protocol::FactRow;
use buizz_core::time::Timestamp;
use buizz_core::visibility::filing_visible_at;
use chrono::{NaiveDate, NaiveTime};
use duckdb::{Connection, Transaction, params, params_from_iter};
use rust_decimal::Decimal;
use serde_json::{Map, Number, Value};

use crate::codec::{Dec, IN_SYMBOLS, Params, Sym, Ts, Wire, symbol_list, text_list, wire_name};
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
     reported AS (SELECT DISTINCT accession, item FROM (SELECT accession, unnest(items) AS item \
     FROM current_report) WHERE item IN (SELECT unnest(?::VARCHAR[]))), \
     in_scope AS (SELECT m.*, '8-K:' || r.item AS section_key FROM main_document m \
     JOIN reported r USING (accession))";

const LOCATED: &str = "SELECT s.doc_id, s.content_hash, s.section_key, s.accession, s.cik, \
     s.published_at, s.volume, s.path, p.start_char, p.end_char FROM in_scope s \
     LEFT JOIN (SELECT doc_id, content_hash, key, start_char, end_char FROM section \
     WHERE key IS NOT NULL QUALIFY splitter_version = max(splitter_version) \
     OVER (PARTITION BY doc_id, content_hash)) p \
     ON p.doc_id = s.doc_id AND p.content_hash = s.content_hash AND p.key = s.section_key \
     WHERE EXISTS (SELECT 1 FROM section x WHERE x.doc_id = s.doc_id \
     AND x.content_hash = s.content_hash)";

const LATEST_BATCH: &str = "SELECT recipe, version, remote_id, state, requests, failed, reserved_usd, recorded_at \
     FROM model_batch QUALIFY row_number() OVER (PARTITION BY remote_id ORDER BY batch_id DESC) = 1";

pub(crate) struct SectionLocation {
    pub(crate) doc_id: String,
    pub(crate) content_hash: String,
    pub(crate) section_key: String,
    pub(crate) accession: Option<String>,
    pub(crate) cik: Option<u32>,
    pub(crate) published_at: Timestamp,
    pub(crate) volume: String,
    pub(crate) path: String,
    pub(crate) chars: Option<(u64, u64)>,
}

pub(crate) struct ExhibitLocation {
    pub(crate) accession: String,
    pub(crate) doc_id: String,
    pub(crate) content_hash: String,
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
        let transaction = connection.transaction()?;
        sync_views(&transaction)?;
        transaction.commit()?;
        return Ok(known);
    }
    write_batch(
        connection,
        source,
        dataset,
        Coverage::new(),
        |transaction, batch_id| {
            let rows = append(transaction, &version, batch_id)?;
            sync_views(transaction)?;
            Ok(rows)
        },
    )?;
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
        (RecipeState::Evaluating, RecipeState::Evaluating) if known.gold.is_none() => {}
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
            sync_views(transaction)?;
            Ok(rows)
        },
    )?;
    Ok(())
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

fn located(
    connection: &Connection,
    version: &RecipeVersion,
    filter: &str,
    limit: usize,
) -> Result<Vec<SectionLocation>> {
    let mut statement = connection.prepare(&format!(
        "{IN_SCOPE} SELECT * FROM ({LOCATED}) s WHERE {filter} \
         ORDER BY published_at DESC, doc_id, section_key LIMIT ?"
    ))?;
    let sections = statement
        .query_map(
            params![
                version.scope.from,
                text_list(version.scope.items.iter().map(String::as_str)),
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
                    chars: row
                        .get::<_, Option<u64>>(8)?
                        .zip(row.get::<_, Option<u64>>(9)?),
                })
            },
        )?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(sections)
}

pub(crate) fn unread_sections(
    connection: &Connection,
    version: &RecipeVersion,
    limit: usize,
) -> Result<Vec<SectionLocation>> {
    let (recipe, number) = (&version.recipe, version.version);
    located(
        connection,
        version,
        &format!(
            "NOT EXISTS (SELECT 1 FROM fact x WHERE x.recipe = '{recipe}' AND x.version = {number} \
             AND x.doc_id = s.doc_id AND x.section_key = s.section_key) \
             AND NOT EXISTS (SELECT 1 FROM model_request q JOIN ({LATEST_BATCH}) b \
             ON b.remote_id = q.remote_id WHERE b.state = 'submitted' AND b.recipe = '{recipe}' \
             AND b.version = {number} AND q.doc_id = s.doc_id AND q.section_key = s.section_key)"
        ),
        limit,
    )
}

pub(crate) fn requested_sections(
    connection: &Connection,
    version: &RecipeVersion,
    remote_id: &str,
) -> Result<(Vec<ModelRequest>, Vec<SectionLocation>)> {
    let mut statement = connection.prepare(
        "SELECT custom_id, doc_id, section_key FROM model_request WHERE remote_id = ? \
         ORDER BY custom_id",
    )?;
    let requests = statement
        .query_map([remote_id], |row| {
            Ok(ModelRequest {
                custom_id: row.get(0)?,
                doc_id: row.get(1)?,
                section_key: row.get(2)?,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    let quoted = remote_id.replace('\'', "''");
    let sections = located(
        connection,
        version,
        &format!(
            "EXISTS (SELECT 1 FROM model_request q WHERE q.remote_id = '{quoted}' \
             AND q.doc_id = s.doc_id AND q.section_key = s.section_key)"
        ),
        requests.len(),
    )?;
    Ok((requests, sections))
}

pub(crate) fn exhibits(
    connection: &Connection,
    accessions: &[&str],
    kinds: &[String],
) -> Result<Vec<ExhibitLocation>> {
    if kinds.is_empty() || accessions.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare(
        "SELECT d.accession, d.doc_id, d.content_hash, f.volume, f.path FROM text_document d \
         JOIN text_file f ON f.id = d.file_id WHERE d.accession IN (SELECT unnest(?::VARCHAR[])) \
         AND EXISTS (SELECT 1 FROM (SELECT unnest(?::VARCHAR[]) AS prefix) k \
         WHERE starts_with(upper(d.kind), k.prefix)) \
         QUALIFY row_number() OVER (PARTITION BY d.doc_id ORDER BY d.batch_id DESC) = 1 \
         ORDER BY d.accession, d.sequence, d.doc_id",
    )?;
    let found = statement
        .query_map(
            params![
                text_list(accessions.iter().copied()),
                text_list(kinds.iter().map(String::as_str))
            ],
            |row| {
                Ok(ExhibitLocation {
                    accession: row.get(0)?,
                    doc_id: row.get(1)?,
                    content_hash: row.get(2)?,
                    volume: row.get(3)?,
                    path: row.get(4)?,
                })
            },
        )?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(found)
}

pub(crate) fn model_batches(
    connection: &Connection,
    recipe: &str,
    version: u32,
) -> Result<Vec<ModelBatch>> {
    let mut statement = connection.prepare(&format!(
        "SELECT * FROM ({LATEST_BATCH}) WHERE recipe = ? AND version = ? ORDER BY recorded_at, remote_id"
    ))?;
    let batches = statement
        .query_map(params![recipe, version], |row| {
            Ok(ModelBatch {
                recipe: row.get(0)?,
                version: row.get(1)?,
                remote_id: row.get(2)?,
                open: row.get::<_, String>(3)? == "submitted",
                requests: row.get(4)?,
                failed: row.get(5)?,
                reserved: row.get::<_, Dec>(6)?.0,
                at: row.get::<_, Ts>(7)?.0,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(batches)
}

pub(crate) fn committed_spend(connection: &Connection, since: Timestamp) -> Result<Decimal> {
    Ok(connection.query_row(
        &format!(
            "SELECT coalesce((SELECT sum(usd) FROM model_spend WHERE spent_at >= ?), 0) \
             + coalesce((SELECT sum(reserved_usd) FROM ({LATEST_BATCH}) WHERE state = 'submitted'), 0)"
        ),
        [Ts(since)],
        |row| Ok(row.get::<_, Dec>(0)?.0),
    )?)
}

fn append_spend(transaction: &Transaction<'_>, spend: &ModelSpend, batch_id: i64) -> Result<u64> {
    Ok(transaction.execute(
        "INSERT INTO main.model_spend VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            spend.recipe,
            spend.version,
            Ts(spend.spent_at),
            spend.calls,
            spend.input_tokens,
            spend.output_tokens,
            Dec(spend.usd),
            batch_id,
        ],
    )? as u64)
}

fn append_batch(transaction: &Transaction<'_>, batch: &ModelBatch, batch_id: i64) -> Result<u64> {
    Ok(transaction.execute(
        "INSERT INTO main.model_batch VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            batch.recipe,
            batch.version,
            batch.remote_id,
            if batch.open { "submitted" } else { "collected" },
            batch.requests,
            batch.failed,
            Dec(batch.reserved),
            Ts(batch.at),
            batch_id,
        ],
    )? as u64)
}

pub(crate) fn submit_batch(
    connection: &mut Connection,
    source: DataSource,
    dataset: Dataset,
    batch: ModelBatch,
    requests: Vec<ModelRequest>,
) -> Result<WriteSummary> {
    write_batch(
        connection,
        source,
        dataset,
        Coverage::new(),
        |transaction, batch_id| {
            let mut rows = append_batch(transaction, &batch, batch_id)?;
            let mut statement = transaction
                .prepare_cached("INSERT INTO main.model_request VALUES (?, ?, ?, ?, ?)")?;
            for request in &requests {
                rows += statement.execute(params![
                    batch.remote_id,
                    request.custom_id,
                    request.doc_id,
                    request.section_key,
                    batch_id
                ])? as u64;
            }
            Ok(rows)
        },
    )
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
    spend: Option<ModelSpend>,
    collected: Option<ModelBatch>,
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
            if let Some(spend) = &spend {
                append_spend(transaction, spend, batch_id)?;
            }
            if let Some(batch) = &collected {
                append_batch(transaction, batch, batch_id)?;
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

#[derive(Clone, Debug, PartialEq)]
pub struct ReadFact {
    pub row: FactRow,
    pub produced_at: Timestamp,
}

pub(crate) struct FactQuery<'a> {
    pub(crate) recipe: &'a str,
    pub(crate) version: u32,
    pub(crate) symbols: &'a [Symbol],
    pub(crate) since: NaiveDate,
    pub(crate) as_of: Timestamp,
    pub(crate) live: bool,
}

pub(crate) fn read_facts(
    connection: &Connection,
    calendar: &Calendar,
    query: &FactQuery<'_>,
) -> Result<Vec<ReadFact>> {
    let sessions = calendar.sessions();
    let opened = sessions.partition_point(|session| session.open <= query.as_of);
    let Some(last_open) = opened.checked_sub(1).map(|index| sessions[index].open) else {
        return Ok(Vec::new());
    };
    if query.symbols.is_empty() {
        return Ok(Vec::new());
    }
    let version = stored(connection, query.recipe, query.version)?.ok_or_else(|| {
        MarketError::Recipe(format!(
            "{} version {} is not registered",
            query.recipe, query.version
        ))
    })?;
    let decimals = decimal_fields(&version.schema)?;
    let since = Timestamp::at_exchange_time(query.since, NaiveTime::MIN).unwrap_or(Timestamp::MIN);
    let sql = format!(
        "SELECT r.symbol, f.accession, f.status, f.fields, f.evidence, f.published_at, \
         f.produced_at FROM fact f JOIN (SELECT DISTINCT symbol, accession FROM current_report) r \
         ON r.accession = f.accession WHERE f.recipe = ? AND f.version = ? \
         AND f.status IN ('ok', 'not_stated') AND r.symbol {IN_SYMBOLS} \
         AND f.published_at >= ? AND f.published_at < ? AND (NOT ? OR f.produced_at <= ?) \
         ORDER BY r.symbol, f.published_at, f.accession, f.section_key, f.ordinal"
    );
    let params: Params = vec![
        Box::new(query.recipe.to_owned()),
        Box::new(query.version),
        symbol_list(query.symbols),
        Box::new(Ts(since)),
        Box::new(Ts(last_open)),
        Box::new(query.live),
        Box::new(Ts(query.as_of)),
    ];
    let mut statement = connection.prepare_cached(&sql)?;
    let rows = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok((
                row.get::<_, Sym>(0)?.0,
                row.get::<_, String>(1)?,
                row.get::<_, Wire<FactStatus>>(2)?.0,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Ts>(5)?.0,
                row.get::<_, Ts>(6)?.0,
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(
            |(symbol, accession, status, fields, evidence, published_at, produced_at)| {
                let public = filing_visible_at(published_at, calendar).unwrap_or(last_open);
                let available_at = if query.live {
                    public.max(produced_at)
                } else {
                    public
                };
                Ok(ReadFact {
                    row: FactRow {
                        symbol,
                        accession,
                        status,
                        available_at,
                        evidence,
                        fields: typed_fields(fields.as_deref(), &decimals)?,
                    },
                    produced_at,
                })
            },
        )
        .collect()
}

fn decimal_fields(schema: &str) -> Result<Vec<String>> {
    let schema: Value = serde_json::from_str(schema)
        .map_err(|error| MarketError::Recipe(format!("the schema is not JSON: {error}")))?;
    Ok(schema
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, property)| match property.get("type") {
            Some(Value::String(kind)) => kind == "number",
            Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind == "number"),
            _ => false,
        })
        .map(|(name, _)| name.clone())
        .collect())
}

fn typed_fields(fields: Option<&str>, decimals: &[String]) -> Result<Map<String, Value>> {
    let Some(fields) = fields else {
        return Ok(Map::new());
    };
    let unreadable = |reason: String| MarketError::Recipe(format!("fact fields: {reason}"));
    let mut fields: Map<String, Value> =
        serde_json::from_str(fields).map_err(|error| unreadable(error.to_string()))?;
    for name in decimals {
        if let Some(Value::Number(number)) = fields.get(name) {
            let exact = exact(number).ok_or_else(|| unreadable(format!("{name} is {number}")))?;
            fields.insert(name.clone(), Value::String(exact.normalize().to_string()));
        }
    }
    Ok(fields)
}

fn exact(number: &Number) -> Option<Decimal> {
    let written = number.to_string();
    Decimal::from_str(&written)
        .or_else(|_| Decimal::from_scientific(&written))
        .ok()
}
