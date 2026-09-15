//! Timestamp helpers for the write path — caller-supplied stamp reads and
//! dialect-shaped bind values.
//!
//! Split out of [`super`] to keep the write path within the file-size standard.
//! `chrono` serializes a `DateTime<Utc>` as an RFC3339 string by default, so a
//! caller-supplied stamp is read from either that string or a numeric Unix
//! timestamp; the stored form is shaped for the runtime pool dialect.

use chrono::{DateTime, SecondsFormat, Utc};

use crate::types::Value;

/// Read a caller-supplied timestamp column from a serialized model object.
///
/// Returns `None` when the column is absent or `null`, so the write path can
/// fall back to the current time. `chrono` serializes a `DateTime<Utc>` as an
/// RFC3339 string by default; a numeric Unix timestamp is accepted too, matching
/// the alternative `serde` representation.
pub(crate) fn provided_timestamp(
    object: &serde_json::Value,
    column: &str,
) -> Option<DateTime<Utc>> {
    match object.get(column)? {
        serde_json::Value::String(text) => DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|stamp| stamp.with_timezone(&Utc)),
        serde_json::Value::Number(number) => number
            .as_i64()
            .and_then(|seconds| DateTime::from_timestamp(seconds, 0)),
        _ => None,
    }
}

/// The current timestamp as a bind value for `dialect`.
///
/// SQLite compares TEXT datetimes lexicographically, so the stored form is a
/// fixed-width RFC3339 string; Postgres/MySQL receive a native `DateTime<Utc>`
/// bound against their `timestamp`/`datetime` columns. The dialect is supplied
/// by the runtime pool so the shape tracks the live driver, not the compiled
/// feature set.
pub(crate) fn timestamp_value(now: DateTime<Utc>, dialect: &str) -> Value {
    match dialect {
        "sqlite" => Value::Text(now.to_rfc3339_opts(SecondsFormat::Micros, true)),
        _ => Value::Timestamp(now),
    }
}
