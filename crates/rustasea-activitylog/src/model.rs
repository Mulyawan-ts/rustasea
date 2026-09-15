//! `Activity` row model and the read/query API over the `audit_log` table.
//!
//! Reads use the ORM runtime API only ([`rustasea_orm::DbPool::fetch_json`]),
//! so the same statements run on SQLite (tests) and Postgres/MySQL. Rows are
//! decoded from JSON: UUID columns arrive as canonical strings and the
//! `properties` JSONB column arrives either as a decoded JSON object or as a
//! text blob depending on the driver.

use chrono::{SecondsFormat, Utc};
use rustasea_orm::{DbPool, Value};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ActivityError, Result};

/// A persisted audit-log row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Activity {
    /// Row identifier.
    pub id: Uuid,
    /// Batch id grouping related writes, when part of a batch.
    pub batch_uuid: Option<Uuid>,
    /// Logical log channel (defaults to the subject table name).
    pub log_name: String,
    /// Short human-readable description (the operation name).
    pub description: String,
    /// Polymorphic subject type (the model type name).
    pub subject_type: Option<String>,
    /// Polymorphic subject id.
    pub subject_id: Option<Uuid>,
    /// Polymorphic causer type (currently always `None`).
    pub causer_type: Option<String>,
    /// Polymorphic causer id (the actor).
    pub causer_id: Option<String>,
    /// `{ "old": …, "new": …, "changed": […] }` property payload.
    pub properties: Option<serde_json::Value>,
    /// Row creation time (RFC3339).
    pub created_at: String,
    /// Row update time (RFC3339).
    pub updated_at: String,
}

/// Decode an optional UUID column that may arrive as a canonical string.
fn uuid_field(row: &serde_json::Value, key: &str) -> Option<Uuid> {
    row.get(key)
        .and_then(|value| value.as_str())
        .and_then(|text| Uuid::parse_str(text).ok())
}

/// Decode an optional string column.
fn string_field(row: &serde_json::Value, key: &str) -> Option<String> {
    row.get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

/// Decode the `properties` column, accepting an object or a JSON text blob.
fn properties_field(row: &serde_json::Value) -> Option<serde_json::Value> {
    match row.get("properties") {
        Some(serde_json::Value::Null) | None => None,
        Some(serde_json::Value::String(text)) => serde_json::from_str(text).ok(),
        Some(other) => Some(other.clone()),
    }
}

/// Decode a timestamp column to its RFC3339 string form.
fn timestamp_field(row: &serde_json::Value, key: &str) -> String {
    row.get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Decode one `audit_log` row.
pub(crate) fn activity_from_row(row: &serde_json::Value) -> Result<Activity> {
    let id = uuid_field(row, "id")
        .ok_or_else(|| ActivityError::Decode("audit_log row is missing `id`".to_string()))?;
    Ok(Activity {
        id,
        batch_uuid: uuid_field(row, "batch_uuid"),
        log_name: string_field(row, "log_name").unwrap_or_default(),
        description: string_field(row, "description").unwrap_or_default(),
        subject_type: string_field(row, "subject_type"),
        subject_id: uuid_field(row, "subject_id"),
        causer_type: string_field(row, "causer_type"),
        causer_id: string_field(row, "causer_id"),
        properties: properties_field(row),
        created_at: timestamp_field(row, "created_at"),
        updated_at: timestamp_field(row, "updated_at"),
    })
}

/// Decode every row of a result set, skipping undecodable rows is not allowed —
/// the first failure aborts with a typed error.
pub(crate) fn activities_from_rows(rows: &[serde_json::Value]) -> Result<Vec<Activity>> {
    rows.iter().map(activity_from_row).collect()
}

/// The read/query API over the `audit_log` table.
///
/// Construct with [`ActivityQuery::new`] and chain one of the filters, or use
/// the free functions on [`crate::ActivityLogger`] which owns a pool.
pub struct ActivityQuery<'a> {
    pool: &'a DbPool,
    table: String,
    filters: Vec<String>,
    bindings: Vec<Value>,
    limit: Option<u64>,
}

impl<'a> ActivityQuery<'a> {
    /// Start a query over `pool` against the default `audit_log` table.
    pub fn new(pool: &'a DbPool) -> Self {
        Self {
            pool,
            table: crate::AUDIT_LOG_TABLE.to_string(),
            filters: Vec::new(),
            bindings: Vec::new(),
            limit: None,
        }
    }

    /// Override the backing table name.
    pub fn with_table(mut self, table: impl Into<String>) -> Self {
        self.table = table.into();
        self
    }

    /// Restrict to rows whose subject matches `(subject_type, subject_id)`.
    pub fn for_subject(mut self, subject_type: &str, subject_id: Uuid) -> Self {
        self.bindings.push(Value::Text(subject_type.to_string()));
        let index = self.bindings.len();
        self.filters.push(format!("subject_type = ${index}"));
        self.bindings.push(Value::Uuid(subject_id));
        let index = self.bindings.len();
        self.filters.push(format!("subject_id = ${index}"));
        self
    }

    /// Restrict to rows caused by `causer_id`.
    pub fn caused_by(mut self, causer_id: &str) -> Self {
        self.bindings.push(Value::Text(causer_id.to_string()));
        let index = self.bindings.len();
        self.filters.push(format!("causer_id = ${index}"));
        self
    }

    /// Restrict to rows in `log_name`.
    pub fn in_log(mut self, log_name: &str) -> Self {
        self.bindings.push(Value::Text(log_name.to_string()));
        let index = self.bindings.len();
        self.filters.push(format!("log_name = ${index}"));
        self
    }

    /// Restrict to rows in `batch_uuid`.
    pub fn for_batch(mut self, batch_uuid: Uuid) -> Self {
        self.bindings.push(Value::Uuid(batch_uuid));
        let index = self.bindings.len();
        self.filters.push(format!("batch_uuid = ${index}"));
        self
    }

    /// Cap the number of rows returned.
    pub fn limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Execute the query, returning rows newest-first.
    pub async fn get(self) -> Result<Vec<Activity>> {
        let mut sql = format!("SELECT * FROM {}", self.table);
        if !self.filters.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.filters.join(" AND "));
        }
        sql.push_str(" ORDER BY created_at DESC");
        if let Some(limit) = self.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }
        let rows = self.pool.fetch_json(&sql, &self.bindings).await?;
        activities_from_rows(&rows)
    }
}

/// Current UTC instant as an RFC3339 micros string.
pub(crate) fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Build the `created_at`/`updated_at` bind shaped for the pool dialect.
pub(crate) fn timestamp_bind(pool: &DbPool) -> Value {
    match pool.dialect() {
        "sqlite" => Value::Text(now_rfc3339()),
        _ => Value::Timestamp(Utc::now()),
    }
}
