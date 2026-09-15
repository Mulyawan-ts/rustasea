//! Composite-primary-key write path — insert/upsert/update/delete emission.
//!
//! Split out of [`super`] to keep the write path within the file-size standard.
//! A composite key addresses a row with a conjunction
//! (`WHERE tenant_id = $1 AND user_id = $2`) instead of a single `id`, and the
//! upsert conflicts on the full key. The single-key helpers in
//! [`super::write`] remain the default path; these builders are only reached
//! when [`Model::has_composite_primary_key`] is true.

use chrono::Utc;
use serde::Serialize;

use crate::error::Result;
use crate::m2::UpsertBuilder;
use crate::model::Model;
use crate::types::Value;

use super::key::KeyFilter;
use super::timestamps::{provided_timestamp, timestamp_value};
use super::write::{
    columns_from_object_excluding, model_object, mysql_upsert_sql_excluding, user_columns_excluding,
};

/// Collect the insertable `(column, value)` pairs for a composite-key model.
///
/// The primary-key columns come first, bound from
/// [`Model::primary_key_values`] in [`Model::primary_key_columns`] order; the
/// remaining user columns (excluding the key columns) and tracked timestamps
/// follow. A caller-supplied timestamp is preserved, matching the single-key
/// contract.
fn composite_columns_and_bindings<T: Model + Serialize>(
    data: &T,
    dialect: &str,
) -> Result<(Vec<String>, Vec<Value>)> {
    let pk_columns = T::primary_key_columns();
    let object = model_object(data)?;
    let columns = columns_from_object_excluding(&object, pk_columns)?;
    let timestamps = T::insert_columns();

    let mut names: Vec<String> = Vec::with_capacity(pk_columns.len() + columns.len());
    let mut bindings: Vec<Value> = Vec::with_capacity(pk_columns.len() + columns.len());
    for (column, value) in pk_columns.iter().zip(data.primary_key_values()) {
        names.push((*column).to_string());
        bindings.push(value);
    }
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

/// Build the INSERT statement and bindings for a composite-key model.
pub(crate) fn build_insert<T: Model + Serialize>(
    data: &T,
    dialect: &str,
) -> Result<(String, Vec<Value>)> {
    let table = T::table_name();
    let (names, bindings) = composite_columns_and_bindings(data, dialect)?;
    let placeholders: Vec<String> = (1..=bindings.len()).map(|i| format!("${i}")).collect();
    let sql = format!(
        "INSERT INTO {table} ({}) VALUES ({})",
        names.join(", "),
        placeholders.join(", ")
    );
    Ok((sql, bindings))
}

/// Build the atomic upsert statement for a composite-key model.
///
/// The conflict target is the full primary key, so
/// `INSERT … ON CONFLICT (col1, col2) DO UPDATE SET …` refreshes a row on an
/// exact key match. MySQL uses `ON DUPLICATE KEY UPDATE`, preserving the key
/// columns and `created_at` on conflict.
pub(crate) fn build_upsert<T: Model + Serialize>(
    data: &T,
    dialect: &str,
) -> Result<(String, Vec<Value>)> {
    let table = T::table_name();
    let (names, bindings) = composite_columns_and_bindings(data, dialect)?;
    let pk_columns = T::primary_key_columns();

    let mut builder = UpsertBuilder::table(table.clone()).unique_by(pk_columns)?;
    for name in &names {
        builder = builder.column(name);
    }
    builder = builder.exclude(&["created_at"]);

    let sql = match dialect {
        "mysql" => mysql_upsert_sql_excluding(&table, &names, pk_columns),
        _ => T::upsert_sql(&builder)?,
    };
    Ok((sql, bindings))
}

/// Build the UPDATE statement and bindings for a composite-key model.
///
/// User columns are assigned `$1..$n` and the key columns filter the tail
/// (`WHERE col1 = $n+1 AND col2 = $n+2`); `updated_at` is appended when tracked.
pub(crate) fn build_update<T: Model + Serialize>(
    data: &T,
    dialect: &str,
) -> Result<(String, Vec<Value>)> {
    let table = T::table_name();
    let pk_columns = T::primary_key_columns();
    let columns = user_columns_excluding(data, pk_columns)?;

    let mut bindings: Vec<Value> = Vec::with_capacity(columns.len() + pk_columns.len() + 1);
    let mut assignments: Vec<String> = Vec::with_capacity(columns.len() + 1);
    for (column, value) in &columns {
        bindings.push(value.clone());
        assignments.push(format!("{column} = ${}", bindings.len()));
    }
    if let Some(updated) = T::updated_column() {
        bindings.push(timestamp_value(Utc::now(), dialect));
        assignments.push(format!("{updated} = ${}", bindings.len()));
    }
    let key = KeyFilter::of(data);
    let mut key_bindings = key.values.clone();
    let where_sql = key.where_sql_from(bindings.len());
    bindings.append(&mut key_bindings);

    let sql = format!(
        "UPDATE {table} SET {} WHERE {where_sql}",
        assignments.join(", ")
    );
    Ok((sql, bindings))
}

/// Build the DELETE statement and bindings for a resolved key filter.
pub(crate) fn build_delete<T: Model>(key: &KeyFilter) -> (String, Vec<Value>) {
    let table = T::table_name();
    (
        format!("DELETE FROM {table} WHERE {}", key.where_sql()),
        key.values.clone(),
    )
}

/// Build the soft-delete statement and bindings for a resolved key filter.
///
/// SQLite binds a fixed-width RFC3339 string for the new `deleted_at` (the
/// dialect governs the shape, not the compiled feature set); Postgres/MySQL use
/// the native `NOW()`.
pub(crate) fn build_soft_delete<T: Model>(key: &KeyFilter, dialect: &str) -> (String, Vec<Value>) {
    let table = T::table_name();
    let mut bindings = key.values.clone();
    let expr = match dialect {
        "sqlite" => {
            bindings.push(timestamp_value(Utc::now(), "sqlite"));
            format!("${}", bindings.len())
        }
        _ => "NOW()".to_string(),
    };
    (
        format!(
            "UPDATE {table} SET deleted_at = {expr} WHERE {}",
            key.where_sql()
        ),
        bindings,
    )
}
