//! Row decoding helpers for the `job_batches` table.
//!
//! Split out of `batch_db.rs` to keep that module within the file-size
//! standard. Every helper is `pub(crate)` and re-used by the repository methods
//! (`find`, `create`) and the failure-recording transaction; the canonical
//! RFC3339 formatting is shared with the timestamp writes so reads and writes
//! compare lexicographically on every driver.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// A persisted batch row from the `job_batches` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchRecord {
    /// Batch identifier (UUID text).
    pub id: String,
    /// Human-readable batch name.
    pub name: String,
    /// Number of jobs in the batch.
    pub total_jobs: i64,
    /// Number of jobs still pending.
    pub pending_jobs: i64,
    /// Number of jobs that failed.
    pub failed_jobs: i64,
    /// Identifiers of the jobs that failed.
    pub failed_job_ids: Vec<String>,
    /// Opaque batch options (serialized JSON).
    pub options: serde_json::Value,
    /// UTC instant the batch was created.
    pub created_at: DateTime<Utc>,
    /// UTC instant the batch finished, `None` while in flight.
    pub finished_at: Option<DateTime<Utc>>,
    /// UTC instant the batch was cancelled, `None` when not cancelled.
    pub cancelled_at: Option<DateTime<Utc>>,
}

impl BatchRecord {
    /// Whether the batch has been marked finished.
    pub fn is_finished(&self) -> bool {
        self.finished_at.is_some()
    }

    /// Whether the batch has recorded at least one failure.
    pub fn has_failures(&self) -> bool {
        self.failed_jobs > 0
    }

    /// Whether the batch was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled_at.is_some()
    }

    /// Whether every job has reached a terminal outcome.
    pub fn is_complete(&self) -> bool {
        self.pending_jobs <= 0
    }
}

/// Current UTC instant as an RFC3339 micros string.
pub(crate) fn rfc3339(instant: DateTime<Utc>) -> String {
    instant.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Decode a [`BatchRecord`] from a `job_batches` row.
pub(crate) fn batch_from_row(row: &serde_json::Value) -> Result<Option<BatchRecord>> {
    let Some(id) = row.get("id").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    Ok(Some(BatchRecord {
        id: id.to_string(),
        name: string_field(row, "name"),
        total_jobs: int_field(row, "total_jobs"),
        pending_jobs: int_field(row, "pending_jobs"),
        failed_jobs: int_field(row, "failed_jobs"),
        failed_job_ids: parse_ids(row.get("failed_job_ids")),
        options: row
            .get("options")
            .and_then(|v| v.as_str())
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or(serde_json::Value::Null),
        created_at: parse_ts(row.get("created_at")).unwrap_or_else(Utc::now),
        finished_at: parse_ts(row.get("finished_at")),
        cancelled_at: parse_ts(row.get("cancelled_at")),
    }))
}

/// Read an integer column, defaulting to zero.
pub(crate) fn int_field(row: &serde_json::Value, key: &str) -> i64 {
    row.get(key).and_then(|v| v.as_i64()).unwrap_or(0)
}

/// Read a string column, defaulting to an empty string.
pub(crate) fn string_field(row: &serde_json::Value, key: &str) -> String {
    row.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Parse an optional RFC3339 timestamp column.
pub(crate) fn parse_ts(value: Option<&serde_json::Value>) -> Option<DateTime<Utc>> {
    value
        .and_then(|v| v.as_str())
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

/// Parse a JSON-encoded `failed_job_ids` array, defaulting to empty.
pub(crate) fn parse_ids(value: Option<&serde_json::Value>) -> Vec<String> {
    let Some(text) = value.and_then(|v| v.as_str()) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(text).unwrap_or_default()
}

/// Read the raw `failed_job_ids` text exactly as stored.
///
/// Used as the optimistic-lock compare value in
/// [`super::DatabaseBatchRepository::record_failure`]: the appended update is
/// guarded on the unchanged prior text, so a concurrent append cannot be
/// overwritten. Returns `None` when the column is absent or not text.
pub(crate) fn raw_ids_text(row: &serde_json::Value) -> Option<String> {
    row.get("failed_job_ids")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}
