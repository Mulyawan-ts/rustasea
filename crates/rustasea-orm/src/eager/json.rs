//! Eager loaders for the JSON-backed relation kinds (ADOPT-020).
//!
//! `staudenmeir/eloquent-json-relations` parity: a relation's keys live inside
//! a JSON column on the owning table rather than in a foreign-key column or a
//! pivot. Split out of [`crate::eager`] to keep that module within the
//! file-size standard; the shared row/grouping helpers are reused from there.
//!
//! Each loader runs exactly one batched `IN (…)` query, never one query per
//! parent. A row whose JSON cell is missing, unparseable, or the wrong shape is
//! **skipped** (it contributes no key and gets a `null`/`[]` payload) rather
//! than failing the whole query — malformed embedded data never aborts eager
//! loading for the healthy rows.

use super::{insert_relation, key_to_value, row_key};
use crate::builder::{Executor, QueryBuilder};
use crate::error::{OrmError, Result};
use crate::model::{JsonSpec, Relation};
use crate::types::Value;
use serde_json::Value as JsonValue;
use std::collections::HashMap;

/// Eager-load a `BelongsToJson`: one parent object per row's embedded scalar id.
pub(crate) async fn load_belongs_to_json<'a>(
    rows: &mut [JsonValue],
    executor: &mut Executor<'a>,
    relation: &Relation,
) -> Result<()> {
    let spec = json_spec(relation)?;
    let mut ids: HashMap<String, Value> = HashMap::new();
    for row in rows.iter() {
        if let Some(id) = scalar_id(row, spec) {
            ids.entry(id.clone()).or_insert_with(|| key_to_value(&id));
        }
    }
    let by_id = fetch_by_id(executor, &relation.related_table, ids).await?;
    for row in rows.iter_mut() {
        let payload = scalar_id(row, spec)
            .and_then(|key| by_id.get(&key).cloned())
            .unwrap_or(JsonValue::Null);
        insert_relation(row, &relation.name, payload);
    }
    Ok(())
}

/// Eager-load a `HasManyJson` / `BelongsToManyJson`: an array of embedded ids.
pub(crate) async fn load_has_many_json<'a>(
    rows: &mut [JsonValue],
    executor: &mut Executor<'a>,
    relation: &Relation,
) -> Result<()> {
    let spec = json_spec(relation)?;
    let mut ids: HashMap<String, Value> = HashMap::new();
    for row in rows.iter() {
        for id in array_ids(row, spec) {
            ids.entry(id.clone()).or_insert_with(|| key_to_value(&id));
        }
    }
    let by_id = fetch_by_id(executor, &relation.related_table, ids).await?;
    for row in rows.iter_mut() {
        let payload: Vec<JsonValue> = array_ids(row, spec)
            .iter()
            .filter_map(|id| by_id.get(id).cloned())
            .collect();
        insert_relation(row, &relation.name, JsonValue::Array(payload));
    }
    Ok(())
}

/// Fetch related rows by `id IN (…)`, keyed by their `id` column.
///
/// Returns an empty map without touching the database when no ids were
/// collected, so an all-malformed parent set still attaches `null`/`[]`.
async fn fetch_by_id<'a>(
    executor: &mut Executor<'a>,
    table: &str,
    ids: HashMap<String, Value>,
) -> Result<HashMap<String, JsonValue>> {
    let mut by_id: HashMap<String, JsonValue> = HashMap::new();
    if ids.is_empty() {
        return Ok(by_id);
    }
    let builder =
        QueryBuilder::table(table.to_string()).where_in("id", ids.into_values().collect());
    let sql = builder.to_sql()?;
    let bindings = builder.bindings().to_vec();
    for row in executor.fetch_json(&sql, &bindings).await? {
        if let Some(key) = row_key(&row, "id") {
            by_id.entry(key).or_insert(row);
        }
    }
    Ok(by_id)
}

/// The relation's JSON spec, or a typed error when the declaration is missing it.
fn json_spec(relation: &Relation) -> Result<&JsonSpec> {
    relation.json_spec().ok_or_else(|| {
        OrmError::InvalidState(format!(
            "JSON relation `{}` is missing a JSON column/path spec",
            relation.name
        ))
    })
}

/// Read one row's embedded scalar id (string or number), skipping malformed data.
fn scalar_id(row: &JsonValue, spec: &JsonSpec) -> Option<String> {
    match read_cell(row, spec)? {
        JsonValue::String(text) => Some(text),
        JsonValue::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// Read one row's embedded array of ids, skipping missing/non-array/malformed
/// elements rather than erroring.
fn array_ids(row: &JsonValue, spec: &JsonSpec) -> Vec<String> {
    match read_cell(row, spec) {
        Some(JsonValue::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                JsonValue::String(text) => Some(text.clone()),
                JsonValue::Number(number) => Some(number.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Resolve a row's JSON cell at `spec.path`, or `None` when missing/unparseable.
///
/// SQLite stores JSON as `TEXT` (decoded to a JSON string), while Postgres and
/// MySQL hand back an already-parsed document; a string cell is therefore
/// re-parsed and any parse failure yields `None` (the row is skipped).
fn read_cell(row: &JsonValue, spec: &JsonSpec) -> Option<JsonValue> {
    let parsed = match row.get(&spec.column)? {
        JsonValue::String(text) => serde_json::from_str(text).ok()?,
        JsonValue::Null => return None,
        other => other.clone(),
    };
    json_at_path(&parsed, &spec.path).cloned()
}

/// Navigate a dot-path into a JSON document, returning the value at the path.
fn json_at_path<'a>(value: &'a JsonValue, path: &str) -> Option<&'a JsonValue> {
    let mut current = value;
    for segment in path.split('.') {
        current = match current {
            JsonValue::Object(map) => map.get(segment)?,
            JsonValue::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}
