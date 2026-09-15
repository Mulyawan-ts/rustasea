//! `AuthenticationLog` row model and the read/query API over the
//! `authentication_log` table.
//!
//! Reads use the ORM runtime API only ([`rustasea_orm::DbPool::fetch_json`]), so
//! the same statements run on SQLite (tests) and Postgres/MySQL. Rows are
//! decoded from JSON: the UUID `id` arrives as a canonical string and the
//! timestamp columns arrive as RFC3339 strings (SQLite stores them as TEXT,
//! Postgres as `TIMESTAMPTZ` which the ORM renders back to RFC3339).

use chrono::{SecondsFormat, Utc};
use rustasea_orm::{DbPool, Value};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AuthLogError, Result};

/// A persisted authentication-log row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthenticationLog {
    /// Row identifier.
    pub id: Uuid,
    /// Authenticated user id, when known.
    pub user_id: Option<String>,
    /// Login identifier (email), when known.
    pub email: Option<String>,
    /// Guard name that produced the event.
    pub guard_name: Option<String>,
    /// Event kind string (`login_succeeded`, `login_failed`, `lockout`,
    /// `logout`).
    pub event: String,
    /// Client IP address, when resolvable.
    pub ip_address: Option<String>,
    /// Client `User-Agent` header, when present.
    pub user_agent: Option<String>,
    /// Whether the attempt authenticated successfully.
    pub successful: bool,
    /// Login time (RFC3339), set on login attempts.
    pub login_at: Option<String>,
    /// Logout time (RFC3339), set when the session was torn down.
    pub logout_at: Option<String>,
    /// When the user cleared their own history (rappasoft parity).
    pub cleared_by_user_at: Option<String>,
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

/// Decode a boolean column, accepting a native bool or an integer `0`/`1`
/// (SQLite stores `BOOLEAN` as an integer).
fn bool_field(row: &serde_json::Value, key: &str) -> bool {
    match row.get(key) {
        Some(serde_json::Value::Bool(flag)) => *flag,
        Some(serde_json::Value::Number(number)) => number.as_i64().unwrap_or(0) != 0,
        Some(serde_json::Value::String(text)) => matches!(text.as_str(), "1" | "true" | "TRUE"),
        _ => false,
    }
}

/// Decode a timestamp column to its RFC3339 string form.
fn timestamp_field(row: &serde_json::Value, key: &str) -> String {
    row.get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Decode one `authentication_log` row.
pub(crate) fn authentication_log_from_row(row: &serde_json::Value) -> Result<AuthenticationLog> {
    let id = uuid_field(row, "id").ok_or_else(|| {
        AuthLogError::Storage("authentication_log row is missing `id`".to_string())
    })?;
    Ok(AuthenticationLog {
        id,
        user_id: string_field(row, "user_id"),
        email: string_field(row, "email"),
        guard_name: string_field(row, "guard_name"),
        event: string_field(row, "event").unwrap_or_default(),
        ip_address: string_field(row, "ip_address"),
        user_agent: string_field(row, "user_agent"),
        successful: bool_field(row, "successful"),
        login_at: string_field(row, "login_at"),
        logout_at: string_field(row, "logout_at"),
        cleared_by_user_at: string_field(row, "cleared_by_user_at"),
        created_at: timestamp_field(row, "created_at"),
        updated_at: timestamp_field(row, "updated_at"),
    })
}

/// Decode every row of a result set; the first failure aborts with a typed error.
pub(crate) fn authentication_logs_from_rows(
    rows: &[serde_json::Value],
) -> Result<Vec<AuthenticationLog>> {
    rows.iter().map(authentication_log_from_row).collect()
}

/// Current UTC instant as an RFC3339 micros string.
pub(crate) fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Build a timestamp bind shaped for the pool dialect.
///
/// SQLite has no native timestamp type, so an RFC3339 string is bound into its
/// `TEXT` column; Postgres binds a real `TIMESTAMPTZ`.
pub(crate) fn timestamp_bind(pool: &DbPool) -> Value {
    match pool.dialect() {
        "sqlite" => Value::Text(now_rfc3339()),
        _ => Value::Timestamp(Utc::now()),
    }
}
