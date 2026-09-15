//! SQL template emission for the [`Model`](crate::model::Model) write path.
//!
//! Split out of [`crate::model`] to keep the trait within the file-size
//! standard. Each function renders one statement template; the trait methods
//! delegate here so the emitted SQL stays in one place. Templates are
//! parameterized with `$n` placeholders — never interpolated values.

use crate::builder::Raw;
use crate::error::Result;
use crate::m2::{InsertBuilder, ModelScopes, UpsertBuilder};

/// Build the raw `SELECT` fragment over `table` for `Model::raw_select`.
///
/// The caller-supplied `sql` is a WHERE fragment spliced verbatim (never a bind
/// value), so it must not be built from user input; it is trimmed before use.
pub(crate) fn raw_select(table: &str, sql: &str) -> Result<Raw> {
    Ok(Raw {
        sql: format!("SELECT * FROM {table} WHERE {}", sql.trim()),
        bindings: Vec::new(),
    })
}

/// Insert SQL: caller columns, dialect-aware conflict guard.
pub(crate) fn insert_sql(builder: &InsertBuilder) -> Result<String> {
    ModelScopes::insert_sql(builder, crate::builder::dialect())
}

/// Upsert SQL with a strict `uniqueBy` (empty → typed error).
pub(crate) fn upsert_sql(builder: &UpsertBuilder) -> Result<String> {
    builder.to_sql()
}

/// Update SQL: `UPDATE t SET <cols> WHERE id = ?`.
///
/// `assignments` are caller-owned `col = ?`-shaped fragments; the primary-key
/// filter and optional `updated_at = now()` bump are appended.
pub(crate) fn update_sql(
    table: &str,
    assignments: &[String],
    updated_column: Option<&str>,
) -> String {
    let mut parts = assignments.to_vec();
    if let Some(col) = updated_column {
        parts.push(format!("{col} = now()"));
    }
    let cols = parts.join(", ");
    format!("UPDATE {table} SET {cols} WHERE id = $1")
}

/// Delete SQL: hard delete by primary key.
pub(crate) fn delete_sql(table: &str) -> String {
    format!("DELETE FROM {table} WHERE id = $1")
}

/// Soft-delete SQL: `UPDATE t SET deleted_at = now() WHERE id = ?`.
pub(crate) fn soft_delete_sql(table: &str) -> String {
    format!("UPDATE {table} SET deleted_at = now() WHERE id = $1")
}

/// Restore SQL: `UPDATE t SET deleted_at = NULL WHERE id = ?`.
pub(crate) fn restore_sql(table: &str) -> String {
    format!("UPDATE {table} SET deleted_at = NULL WHERE id = $1")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The update template appends the timestamp bump when tracked.
    #[test]
    fn update_sql_appends_updated_column() {
        let sql = update_sql("users", &["name = $1".to_string()], Some("updated_at"));
        assert_eq!(
            sql,
            "UPDATE users SET name = $1, updated_at = now() WHERE id = $1"
        );
    }

    /// The update template omits the bump when the model has no timestamp.
    #[test]
    fn update_sql_without_updated_column() {
        let sql = update_sql("users", &["name = $1".to_string()], None);
        assert_eq!(sql, "UPDATE users SET name = $1 WHERE id = $1");
    }

    /// Delete/soft-delete/restore templates render the expected statements.
    #[test]
    fn keyed_templates_render() {
        assert_eq!(delete_sql("users"), "DELETE FROM users WHERE id = $1");
        assert_eq!(
            soft_delete_sql("users"),
            "UPDATE users SET deleted_at = now() WHERE id = $1"
        );
        assert_eq!(
            restore_sql("users"),
            "UPDATE users SET deleted_at = NULL WHERE id = $1"
        );
    }
}
