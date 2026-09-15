//! Row-value `IN` predicates for composite keys (awobaz/compoships parity).
//!
//! Split out of [`crate::builder`] to keep the builder module within the
//! file-size standard. [`QueryBuilder::where_in_rows`] emits the SQL row-value
//! form `(a, b) IN (($1, $2), ($3, $4))`, which every supported driver
//! understands: Postgres natively, SQLite from 3.15, and MySQL from 8.0.19.
//! An empty row set degrades to `1 = 0` so the predicate never becomes an
//! invalid `IN ()`. Placeholders are always `$n`; `db/adapt.rs` rewrites them to
//! `?` for MySQL.

use super::QueryBuilder;
use crate::types::Value;

impl QueryBuilder {
    /// Add a composite `(columns) IN (rows)` clause using SQL row values.
    ///
    /// `columns` are the left-hand columns; each inner `rows` entry is one tuple
    /// of bind values in the same order. Rows whose arity does not match
    /// `columns` are dropped defensively, and an empty (or fully-dropped) set
    /// emits `1 = 0` rather than a malformed `IN ()`.
    pub fn where_in_rows(mut self, columns: &[&str], rows: Vec<Vec<Value>>) -> Self {
        if columns.is_empty() {
            self.push_condition("AND", "1 = 0");
            return self;
        }
        let mut tuples: Vec<String> = Vec::with_capacity(rows.len());
        for row in rows {
            if row.len() != columns.len() {
                continue;
            }
            let placeholders: Vec<String> = row
                .into_iter()
                .map(|value| {
                    self.bindings.push(value);
                    format!("${}", self.bindings.len())
                })
                .collect();
            tuples.push(format!("({})", placeholders.join(", ")));
        }
        if tuples.is_empty() {
            self.push_condition("AND", "1 = 0");
            return self;
        }
        let columns = columns.join(", ");
        self.push_condition("AND", format!("({columns}) IN ({})", tuples.join(", ")));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies a two-column predicate emits the row-value shape with aligned
    /// placeholders and bindings.
    #[test]
    fn emits_row_value_predicate() {
        let builder = QueryBuilder::table("memberships").where_in_rows(
            &["tenant_id", "user_id"],
            vec![
                vec![Value::Text("t1".into()), Value::Uuid(uuid::Uuid::nil())],
                vec![Value::Text("t2".into()), Value::Uuid(uuid::Uuid::nil())],
            ],
        );
        assert_eq!(
            builder.to_sql().unwrap(),
            "SELECT * FROM memberships WHERE (tenant_id, user_id) IN \
             (($1, $2), ($3, $4))"
        );
        assert_eq!(builder.bindings().len(), 4);
    }

    /// Verifies an empty row set degrades to a false predicate.
    #[test]
    fn empty_rows_emit_false_predicate() {
        let builder =
            QueryBuilder::table("memberships").where_in_rows(&["tenant_id", "user_id"], Vec::new());
        assert_eq!(
            builder.to_sql().unwrap(),
            "SELECT * FROM memberships WHERE 1 = 0"
        );
        assert!(builder.bindings().is_empty());
    }

    /// Verifies arity-mismatched rows are dropped rather than emitting
    /// unbalanced tuples.
    #[test]
    fn mismatched_rows_are_dropped() {
        let builder = QueryBuilder::table("memberships").where_in_rows(
            &["tenant_id", "user_id"],
            vec![vec![Value::Text("t1".into())]],
        );
        assert_eq!(
            builder.to_sql().unwrap(),
            "SELECT * FROM memberships WHERE 1 = 0"
        );
    }
}
