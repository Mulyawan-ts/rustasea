//! Database-backed `tinker` verbs — `db.query` and `model`.
//!
//! These are the only REPL verbs that touch a live database. Rather than
//! reading the application's container (which the REPL deliberately cannot
//! reach — see [`super`]), each verb resolves the database URL from the shared
//! configuration seam and opens a short-lived [`DbPool`], mirroring the
//! `migrate` and `queue:failed` commands. The pool is always closed before the
//! verb returns.
//!
//! # Read-only contract
//!
//! `db.query` accepts only `SELECT` / `WITH` / `EXPLAIN` / `PRAGMA` statements;
//! any other leading keyword is refused before a connection is opened, so the
//! REPL can never mutate the database. `model <table>` interpolates the table
//! name into a `COUNT(*)`/`LIMIT` statement, so the identifier is validated
//! against `^[A-Za-z_][A-Za-z0-9_]*$` first — an injection attempt is rejected
//! before any SQL is built.
//!
//! # Error handling
//!
//! Every failure (no URL, connect failure, SQL error) renders as a friendly
//! message; the verb never panics and never returns an error to the REPL loop,
//! so a bad query leaves the session alive.

use crate::commands::ops::database_url;
use rustasea_orm::{DbPool, OrmError};

/// Maximum number of rows rendered before the output is truncated.
const ROW_CAP: usize = 100;

/// Friendly message shown when no database URL can be resolved.
const NO_DATABASE: &str = "db unavailable: database URL not configured — set `database.url` in \
                           config/database.toml or the DATABASE_URL env var";

/// Renderer for `db.query <sql>`.
///
/// The SQL is rebuilt from the whitespace-split arguments joined with single
/// spaces, so inner whitespace collapses. Blank input yields a usage line, a
/// mutation statement is refused, and every other failure renders as a friendly
/// message rather than aborting the REPL.
pub(crate) async fn db_query(args: &[&str]) -> String {
    if args.is_empty() {
        return "usage: db.query <sql>   (read-only; e.g. db.query SELECT * FROM users LIMIT 5)"
            .to_string();
    }
    let sql = args.join(" ");
    if !is_read_only(&sql) {
        return "db.query is read-only; mutation SQL is not supported (use a real client)"
            .to_string();
    }
    match run_query(&sql).await {
        Ok(rows) => render_rows(rows),
        Err(message) => message,
    }
}

/// Renderer for `model <table> [limit]`.
///
/// Without a limit the verb counts the table's rows (`users: 3 rows`); with one
/// it lists up to that many rows as pretty JSON. The table identifier is
/// validated before any SQL is built, so an injection attempt never reaches the
/// database.
pub(crate) async fn model(args: &[&str]) -> String {
    let Some(table) = args.first() else {
        return "usage: model <table> [limit]   (e.g. model users, model users 5)".to_string();
    };
    if !is_valid_identifier(table) {
        return format!("invalid table name `{table}` — use letters, digits and underscores");
    }
    match args.get(1) {
        None => count_rows(table).await,
        Some(raw) => match raw.trim().parse::<usize>() {
            Ok(limit) if limit > 0 => list_rows(table, limit).await,
            _ => format!(
                "usage: model <table> [limit]   (limit must be a positive integer, got `{raw}`)"
            ),
        },
    }
}

/// Keywords that mutate data or schema and are refused inside a `WITH` CTE.
const MUTATING_KEYWORDS: [&str; 6] = ["DELETE", "UPDATE", "INSERT", "DROP", "ALTER", "CREATE"];

/// Whether the leading keyword marks `sql` as a read-only statement.
///
/// Leading SQL comments (`-- ...` line comments and `/* ... */` block comments,
/// possibly repeated) are stripped before the statement keyword is inspected.
/// `SELECT` and `EXPLAIN` are accepted as-is. `WITH` is accepted only when the
/// payload carries no mutating keyword, so a data-modifying CTE (for example
/// `WITH deleted AS (DELETE FROM users RETURNING *) SELECT * FROM deleted`) is
/// refused. `PRAGMA` is accepted only when it carries no `=` assignment, so a
/// mutating statement such as `PRAGMA user_version = 42` is refused while an
/// inspection like `PRAGMA table_info(users)` stays allowed. Every other
/// leading keyword is refused.
fn is_read_only(sql: &str) -> bool {
    let statement = strip_leading_comments(sql);
    let first = statement
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    match first.as_str() {
        "SELECT" | "EXPLAIN" => true,
        "WITH" => !contains_mutating_keyword(statement),
        "PRAGMA" => !statement.contains('='),
        _ => false,
    }
}

/// Strip leading line and block comments from `sql`, repeatedly.
///
/// Only comments before the first statement keyword matter; the remainder of the
/// payload is returned unchanged. An unterminated comment consumes the rest of
/// the input, leaving an empty statement that [`is_read_only`] refuses.
fn strip_leading_comments(sql: &str) -> &str {
    let mut rest = sql.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("--") {
            rest = match after.find('\n') {
                Some(index) => after[index + 1..].trim_start(),
                None => "",
            };
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = match after.find("*/") {
                Some(index) => after[index + 2..].trim_start(),
                None => "",
            };
        } else {
            return rest;
        }
    }
}

/// Whether `sql` contains a mutating keyword as a whole, case-insensitive word.
fn contains_mutating_keyword(sql: &str) -> bool {
    sql.to_ascii_uppercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|token| MUTATING_KEYWORDS.contains(&token))
}

/// Whether `name` is a safe SQL identifier (`^[A-Za-z_][A-Za-z0-9_]*$`).
fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Count the rows of `table` and render `<table>: <n> rows`.
async fn count_rows(table: &str) -> String {
    let sql = format!("SELECT COUNT(*) AS n FROM {table}");
    match run_query(&sql).await {
        Ok(rows) => {
            let count = rows
                .first()
                .and_then(|row| row.get("n"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            format!("{table}: {count} rows")
        }
        Err(message) => message,
    }
}

/// List up to `limit` rows of `table` as pretty JSON.
async fn list_rows(table: &str, limit: usize) -> String {
    let sql = format!("SELECT * FROM {table} LIMIT {limit}");
    match run_query(&sql).await {
        Ok(rows) => render_rows(rows),
        Err(message) => message,
    }
}

/// Open a short-lived pool, run a read-only `sql`, and close the pool.
///
/// Returns the decoded rows or a friendly message. The pool is closed on both
/// the success and failure paths so a REPL session never leaks connections.
async fn run_query(sql: &str) -> Result<Vec<serde_json::Value>, String> {
    let url = database_url().map_err(|_| NO_DATABASE.to_string())?;
    let pool = DbPool::connect(&url)
        .await
        .map_err(|error| format!("db unavailable: {error}"))?;
    let result = pool.query_raw(sql, &[]).await;
    pool.close().await;
    result.map_err(format_orm_error)
}

/// Map an [`OrmError`] onto a friendly REPL message.
///
/// A missing table gets a dedicated message; every other error is prefixed with
/// `db error:` so the operator can distinguish an application-level failure
/// from an unavailable database.
fn format_orm_error(error: OrmError) -> String {
    match error {
        OrmError::MissingTable { table } => format!("table `{table}` does not exist"),
        OrmError::Storage(message) => format!("db error: {message}"),
        other => format!("db error: {other}"),
    }
}

/// Render `rows` as pretty JSON, capping the output at [`ROW_CAP`] rows.
///
/// An empty result renders `(0 rows)`; a truncated result appends a
/// `(showing first N rows)` tail.
fn render_rows(rows: Vec<serde_json::Value>) -> String {
    if rows.is_empty() {
        return "(0 rows)".to_string();
    }
    let truncated = rows.len() > ROW_CAP;
    let shown: Vec<serde_json::Value> = rows.into_iter().take(ROW_CAP).collect();
    let mut out = match serde_json::to_string_pretty(&shown) {
        Ok(text) => text,
        Err(error) => return format!("db error: failed to render rows: {error}"),
    };
    if truncated {
        out.push_str(&format!("\n(showing first {ROW_CAP} rows)"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The read-only guard accepts the query keywords in any case.
    #[test]
    fn read_only_guard_accepts_only_queries() {
        for sql in [
            "SELECT * FROM users",
            "  select 1",
            "WITH t AS (SELECT 1) SELECT * FROM t",
            "EXPLAIN SELECT 1",
            "pragma table_info(users)",
            "PRAGMA table_info(users)",
            "-- comment\nSELECT 1",
            "/* block */ SELECT 1",
            "  -- one\n  /* two */  SELECT 1",
        ] {
            assert!(is_read_only(sql), "should accept: {sql}");
        }
        for sql in [
            "DELETE FROM users",
            "UPDATE users SET x = 1",
            "INSERT INTO users (id) VALUES (1)",
            "DROP TABLE users",
            "ALTER TABLE users ADD COLUMN x INT",
            "WITH deleted AS (DELETE FROM users RETURNING *) SELECT * FROM deleted",
            "with updated as (update users set x = 1 returning *) select * from updated",
            "PRAGMA user_version = 42",
            "pragma user_version=42",
            "-- comment\nDELETE FROM users",
            "/* block */ PRAGMA user_version = 42",
        ] {
            assert!(!is_read_only(sql), "should refuse: {sql}");
        }
    }

    /// A data-modifying CTE is refused; a pure CTE stays accepted.
    #[test]
    fn read_only_guard_rejects_mutating_cte() {
        assert!(!is_read_only(
            "WITH deleted AS (DELETE FROM users RETURNING *) SELECT * FROM deleted"
        ));
        assert!(is_read_only(
            "WITH t AS (SELECT * FROM users) SELECT * FROM t"
        ));
    }

    /// PRAGMA assignments are refused; PRAGMA inspection stays accepted.
    #[test]
    fn read_only_guard_rejects_pragma_assignment() {
        assert!(!is_read_only("PRAGMA user_version = 42"));
        assert!(is_read_only("PRAGMA table_info(users)"));
    }

    /// Leading line and block comments are ignored before keyword extraction.
    #[test]
    fn read_only_guard_strips_leading_comments() {
        assert!(is_read_only("-- comment\nSELECT 1"));
        assert!(is_read_only("/* block */ SELECT 1"));
    }

    /// The identifier guard accepts well-formed names and rejects injection.
    #[test]
    fn identifier_validation_rejects_injection() {
        assert!(is_valid_identifier("users"));
        assert!(is_valid_identifier("_private"));
        assert!(is_valid_identifier("users2"));
        assert!(!is_valid_identifier("1bad"));
        assert!(!is_valid_identifier("users; DROP TABLE users"));
        assert!(!is_valid_identifier("users;"));
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("user table"));
    }

    /// The row renderer caps output at ROW_CAP rows and annotates the tail.
    #[test]
    fn row_renderer_caps_at_hundred() {
        let rows: Vec<serde_json::Value> = (0..150).map(|i| json!({ "n": i })).collect();
        let rendered = render_rows(rows);
        assert!(rendered.contains("(showing first 100 rows)"), "{rendered}");
        assert!(rendered.contains("\"n\": 99"), "{rendered}");
        assert!(!rendered.contains("\"n\": 100"), "{rendered}");
    }

    /// An empty result renders the zero-row marker.
    #[test]
    fn empty_result_renders_zero_rows() {
        assert_eq!(render_rows(Vec::new()), "(0 rows)");
    }

    /// Rows render as pretty JSON (multi-line, spaced separators).
    #[test]
    fn rows_render_as_pretty_json() {
        let rendered = render_rows(vec![json!({ "answer": 42 })]);
        assert!(rendered.contains("\"answer\": 42"), "{rendered}");
        assert!(
            rendered.contains('\n'),
            "pretty output spans lines: {rendered}"
        );
    }
}
