//! Single-key write-path SQL helpers — insert/upsert/update emission and the
//! serde→bind conversion shared by the composite path.
//!
//! Split out of [`super`] to keep the write path within the file-size standard
//! and to give both the single-key and composite builders a shared home. These
//! helpers emit SQL text only; execution is driven by [`super::ModelOps`].

use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use crate::error::{OrmError, Result};
use crate::m2::UpsertBuilder;
use crate::model::Model;
use crate::types::Value;

use super::timestamps::{provided_timestamp, timestamp_value};

/// Fields that are managed by the ORM and never written as user columns.
pub(crate) const RESERVED_COLUMNS: &[&str] =
    &["id", "created_at", "updated_at", "deleted_at", "relations"];

/// Collect the insertable `(column, value)` pairs for a model instance.
///
/// User columns come from the model's `serde` object (reserved fields skipped);
/// the primary key is first and each tracked timestamp column is appended. A
/// timestamp the model already set is preserved; one left unset is filled with
/// the current UTC time — the Laravel `updateTimestamps` contract, where a
/// stamp that is already populated is not overwritten. Shared by
/// [`build_insert`] and [`build_upsert`].
pub(crate) fn insert_columns_and_bindings<T: Model + Serialize>(
    data: &T,
    dialect: &str,
) -> Result<(Vec<String>, Vec<Value>)> {
    let object = model_object(data)?;
    let columns = columns_from_object(&object)?;
    let timestamps = T::insert_columns();

    let mut names: Vec<String> = Vec::with_capacity(columns.len() + timestamps.len() + 1);
    let mut bindings: Vec<Value> = Vec::with_capacity(columns.len() + timestamps.len() + 1);
    names.push("id".to_string());
    bindings.push(Value::Uuid(data.primary_key()));
    for (column, value) in &columns {
        names.push(column.clone());
        bindings.push(value.clone());
    }
    if !timestamps.is_empty() {
        let now = Utc::now();
        for ts in &timestamps {
            let stamp = provided_timestamp(&object, ts).unwrap_or(now);
            names.push((*ts).to_string());
            bindings.push(timestamp_value(stamp, dialect));
        }
    }
    Ok((names, bindings))
}

/// Build the INSERT statement and bindings for a model instance.
pub(crate) fn build_insert<T: Model + Serialize>(
    data: &T,
    dialect: &str,
) -> Result<(String, Vec<Value>)> {
    let table = T::table_name();
    let (names, bindings) = insert_columns_and_bindings(data, dialect)?;
    let placeholders: Vec<String> = (1..=bindings.len()).map(|i| format!("${i}")).collect();
    let sql = format!(
        "INSERT INTO {table} ({}) VALUES ({})",
        names.join(", "),
        placeholders.join(", ")
    );
    Ok((sql, bindings))
}

/// Build the atomic dialect-aware upsert statement and bindings for a model.
///
/// Postgres/SQLite route through the [`Model::upsert_sql`] path, emitting
/// `INSERT … ON CONFLICT (id) DO UPDATE SET …`; MySQL emits
/// `INSERT … ON DUPLICATE KEY UPDATE …`. `created_at` is preserved on conflict
/// (excluded from the update set) while `updated_at` is refreshed. Resolving
/// insert-vs-update in one statement removes the previous check-then-act race.
pub(crate) fn build_upsert<T: Model + Serialize>(
    data: &T,
    dialect: &str,
) -> Result<(String, Vec<Value>)> {
    let table = T::table_name();
    let (names, bindings) = insert_columns_and_bindings(data, dialect)?;

    let mut builder = UpsertBuilder::table(table.clone()).unique_by(&["id"])?;
    for name in &names {
        builder = builder.column(name);
    }
    builder = builder.exclude(&["created_at"]);

    let sql = match dialect {
        "mysql" => mysql_upsert_sql(&table, &names),
        _ => T::upsert_sql(&builder)?,
    };
    Ok((sql, bindings))
}

/// Emit the MySQL upsert shape: `INSERT … ON DUPLICATE KEY UPDATE col = VALUES(col)`.
///
/// The primary key and `created_at` are preserved on conflict; every other
/// inserted column is refreshed from the incoming row.
pub(crate) fn mysql_upsert_sql(table: &str, columns: &[String]) -> String {
    mysql_upsert_sql_excluding(table, columns, &["id"])
}

/// Like [`mysql_upsert_sql`] but preserving an arbitrary set of key `columns`.
pub(crate) fn mysql_upsert_sql_excluding(
    table: &str,
    columns: &[String],
    key_columns: &[&str],
) -> String {
    let placeholders: Vec<String> = (1..=columns.len()).map(|i| format!("${i}")).collect();
    let updates: Vec<String> = columns
        .iter()
        .filter(|column| column.as_str() != "created_at" && !key_columns.contains(&column.as_str()))
        .map(|column| format!("{column} = VALUES({column})"))
        .collect();
    format!(
        "INSERT INTO {table} ({}) VALUES ({}) ON DUPLICATE KEY UPDATE {}",
        columns.join(", "),
        placeholders.join(", "),
        updates.join(", ")
    )
}

/// Build the UPDATE statement and bindings for a model instance.
///
/// The primary key is `$1`; `updated_at` is appended as a bound timestamp shaped
/// for `dialect`. `created_at` is reserved and never reassigned, so an update
/// refreshes `updated_at` without disturbing the creation stamp.
pub(crate) fn build_update<T: Model + Serialize>(
    data: &T,
    dialect: &str,
) -> Result<(String, Vec<Value>)> {
    let table = T::table_name();
    let columns = user_columns(data)?;

    let mut bindings: Vec<Value> = vec![Value::Uuid(data.primary_key())];
    let mut assignments: Vec<String> = Vec::with_capacity(columns.len() + 1);
    for (column, value) in &columns {
        bindings.push(value.clone());
        assignments.push(format!("{column} = ${}", bindings.len()));
    }
    if let Some(updated) = T::updated_column() {
        bindings.push(timestamp_value(Utc::now(), dialect));
        assignments.push(format!("{updated} = ${}", bindings.len()));
    }

    let sql = format!(
        "UPDATE {table} SET {} WHERE id = $1",
        assignments.join(", ")
    );
    Ok((sql, bindings))
}

/// Serialize a model to its JSON object with persistence casts applied.
///
/// Every declared attribute cast is applied first (in the persistence
/// direction), so a `#[model(cast = "json")]` field is already JSON rather than
/// its raw serde shape. The owned object is returned so callers can both read a
/// caller-supplied timestamp and derive the bind columns from it.
pub(crate) fn model_object<T: Model + Serialize>(data: &T) -> Result<serde_json::Value> {
    let mut value = serde_json::to_value(data)
        .map_err(|error| OrmError::Storage(format!("model serialization failed: {error}")))?;
    apply_set_casts::<T>(&mut value)?;
    if !value.is_object() {
        return Err(OrmError::InvalidValue(
            "model must serialize to a JSON object".into(),
        ));
    }
    Ok(value)
}

/// Extract the writable `(column, value)` pairs from a serialized model object.
///
/// Reserved columns (`id`, the timestamps, `deleted_at`, `relations`) are
/// skipped: the primary key and timestamps are managed by the write path.
pub(crate) fn columns_from_object(object: &serde_json::Value) -> Result<Vec<(String, Value)>> {
    columns_from_object_excluding(object, RESERVED_COLUMNS)
}

/// Like [`columns_from_object`] but skipping an extra set of `excluded` columns.
///
/// The composite write path passes its primary-key columns so they are bound
/// from [`Model::primary_key_values`] (type-faithful) rather than coerced by the
/// `*_id` UUID heuristic in [`json_to_value`].
pub(crate) fn columns_from_object_excluding(
    object: &serde_json::Value,
    excluded: &[&str],
) -> Result<Vec<(String, Value)>> {
    let object = object
        .as_object()
        .ok_or_else(|| OrmError::InvalidValue("model must serialize to a JSON object".into()))?;

    let mut columns = Vec::new();
    for (column, value) in object {
        if RESERVED_COLUMNS.contains(&column.as_str()) || excluded.contains(&column.as_str()) {
            continue;
        }
        columns.push((column.clone(), json_to_value(column, value)?));
    }
    Ok(columns)
}

/// Extract the writable `(column, value)` pairs from a model's serde object.
///
/// Every declared attribute cast is applied first (in the persistence
/// direction), so a `#[model(cast = "json")]` field binds as JSON rather than
/// as its raw serde shape.
pub(crate) fn user_columns<T: Model + Serialize>(data: &T) -> Result<Vec<(String, Value)>> {
    let object = model_object(data)?;
    columns_from_object(&object)
}

/// Like [`user_columns`] but skipping an extra set of `excluded` columns.
pub(crate) fn user_columns_excluding<T: Model + Serialize>(
    data: &T,
    excluded: &[&str],
) -> Result<Vec<(String, Value)>> {
    let object = model_object(data)?;
    columns_from_object_excluding(&object, excluded)
}

/// Apply the persistence direction of every cast declared on `T` in place.
///
/// Columns absent from the serialized object are skipped, so an optional cast
/// field that is `None` (and therefore omitted) does not error.
fn apply_set_casts<T: Model>(value: &mut serde_json::Value) -> Result<()> {
    let bindings = T::casts();
    if bindings.is_empty() {
        return Ok(());
    }
    let object = value
        .as_object_mut()
        .ok_or_else(|| OrmError::InvalidValue("model must serialize to a JSON object".into()))?;
    for binding in &bindings {
        if let Some(field) = object.get(binding.column).cloned() {
            let bound = (binding.set)(binding.column, &field)?;
            object.insert(binding.column.to_string(), bound.to_json());
        }
    }
    Ok(())
}

/// Whether `column` holds a UUID and therefore must bind natively as one.
///
/// The primary key (`id`) and foreign keys/UUID columns (`*_id`, `*_uuid`) are
/// covered; every other string column stays text.
fn is_uuid_column(column: &str) -> bool {
    column == "id" || column.ends_with("_id") || column.ends_with("_uuid")
}

/// Convert a JSON field value into a bind [`Value`].
///
/// The primary key and `*_id`/`*_uuid` columns are decoded to a UUID so they
/// bind against UUID columns; a string that fails to parse in one of those
/// columns is a typed [`OrmError::InvalidValue`] rather than a silent text bind.
pub(crate) fn json_to_value(column: &str, value: &serde_json::Value) -> Result<Value> {
    Ok(match value {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(number) => match number.as_i64() {
            Some(int) => Value::Int(int),
            None => Value::Float(number.as_f64().unwrap_or_default()),
        },
        serde_json::Value::String(text) => {
            if is_uuid_column(column) {
                match Uuid::parse_str(text) {
                    Ok(id) => return Ok(Value::Uuid(id)),
                    Err(_) => {
                        return Err(OrmError::InvalidValue(format!(
                            "column `{column}` expects a UUID, got `{text}`"
                        )))
                    }
                }
            }
            Value::Text(text.clone())
        }
        other => Value::Json(other.clone()),
    })
}
