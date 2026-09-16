//! JSON-embedded key predicates for `*_json` relations (ADOPT-020).
//!
//! Split out of [`crate::builder`] to keep the builder module within the
//! file-size standard. [`QueryBuilder::where_json_in`] matches a single JSON
//! value at a path against a set of keys (`BelongsToJson`), while
//! [`QueryBuilder::where_json_contains_any`] matches a JSON **array** at a path
//! containing any of the keys (`HasManyJson` / `BelongsToManyJson`). Both emit
//! `$n` placeholders (rewritten to `?` for MySQL by `db/adapt.rs`).
//!
//! An empty value list degrades to `1 = 0` (matching [`QueryBuilder::where_in`]),
//! so a caller never builds a malformed `IN ()`.

use super::{dialect, QueryBuilder};
use crate::error::Result;
use crate::types::{JsonRelationFilter, Value};

impl QueryBuilder {
    /// Add a scalar JSON-key `IN` clause (`BelongsToJson` semantics).
    ///
    /// The value at `path` inside `column` is compared against `values`. An
    /// empty `values` list emits `1 = 0`.
    pub fn where_json_in(mut self, column: &str, path: &str, values: &[String]) -> Result<Self> {
        if values.is_empty() {
            self.push_condition("AND", "1 = 0");
            return Ok(self);
        }
        let filter = JsonRelationFilter {
            path: path.to_string(),
            values: values.to_vec(),
        };
        let (fragment, _) = filter.to_sql(column, dialect())?;
        let sql =
            self.bind_placeholders(&fragment, values.iter().cloned().map(Value::Text).collect());
        self.push_condition("AND", sql);
        Ok(self)
    }

    /// Add an array-overlap JSON clause (`HasManyJson` / `BelongsToManyJson`).
    ///
    /// The array at `path` inside `column` must contain any of `values`. An
    /// empty `values` list emits `1 = 0`.
    pub fn where_json_contains_any(
        mut self,
        column: &str,
        path: &str,
        values: &[String],
    ) -> Result<Self> {
        if values.is_empty() {
            self.push_condition("AND", "1 = 0");
            return Ok(self);
        }
        let filter = JsonRelationFilter {
            path: path.to_string(),
            values: values.to_vec(),
        };
        let (fragment, _) = filter.to_sql_array(column, dialect())?;
        // MySQL binds the candidate set as ONE JSON-array literal; the other
        // dialects bind each value as its own placeholder.
        let binds = if dialect() == "mysql" {
            let array = serde_json::Value::Array(
                values
                    .iter()
                    .map(|value| serde_json::Value::String(value.clone()))
                    .collect(),
            );
            vec![Value::Json(array)]
        } else {
            values.iter().cloned().map(Value::Text).collect()
        };
        let sql = self.bind_placeholders(&fragment, binds);
        self.push_condition("AND", sql);
        Ok(self)
    }

    /// Substitute each `{}` token in `fragment` with the next `$n` placeholder,
    /// pushing `binds` onto this builder's binding list.
    ///
    /// The number of tokens must match `binds.len()`; the fragment comes from a
    /// trusted [`JsonRelationFilter`], so a mismatch is an internal invariant and
    /// any leftover token is left untouched (it would surface as invalid SQL).
    fn bind_placeholders(&mut self, fragment: &str, binds: Vec<Value>) -> String {
        let mut out = String::with_capacity(fragment.len());
        let mut rest = fragment;
        for value in binds {
            self.bindings.push(value);
            let placeholder = format!("${}", self.bindings.len());
            match rest.find("{}") {
                Some(index) => {
                    out.push_str(&rest[..index]);
                    out.push_str(&placeholder);
                    rest = &rest[index + 2..];
                }
                None => {
                    // No token left: keep the remainder verbatim.
                    out.push_str(rest);
                    rest = "";
                }
            }
        }
        out.push_str(rest);
        out
    }
}
