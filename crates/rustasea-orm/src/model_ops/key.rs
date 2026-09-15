//! Primary-key filters shared by the single-key and composite write paths.
//!
//! [`KeyFilter`] resolves a model's declared [`Model::primary_key_columns`] to
//! the matching bind values ([`Model::primary_key_values`]), so the write path
//! can address a row by one column or a composite tuple without branching on
//! the key shape at every call site. The single `id` key is the special case
//! `columns = ["id"]`.

use crate::builder::QueryBuilder;
use crate::model::Model;
use crate::types::Value;

/// A resolved primary-key filter over one or more columns.
#[derive(Debug, Clone)]
pub(crate) struct KeyFilter {
    /// Primary-key column names.
    pub(crate) columns: Vec<&'static str>,
    /// Bind values aligned with `columns`.
    pub(crate) values: Vec<Value>,
}

impl KeyFilter {
    /// The filter for a model instance's declared primary key.
    pub(crate) fn of<T: Model>(data: &T) -> Self {
        Self {
            columns: T::primary_key_columns().to_vec(),
            values: data.primary_key_values(),
        }
    }

    /// Apply this filter to a query builder as `col = $n AND …` conditions.
    pub(crate) fn apply(&self, builder: QueryBuilder) -> QueryBuilder {
        let mut builder = builder;
        for (column, value) in self.columns.iter().zip(self.values.iter()) {
            builder = builder.where_eq(column, value.clone());
        }
        builder
    }

    /// The `WHERE col1 = $1 AND col2 = $2` clause with `$n` placeholders.
    pub(crate) fn where_sql(&self) -> String {
        self.where_sql_from(0)
    }

    /// Like [`KeyFilter::where_sql`] but starting placeholders at `offset`.
    ///
    /// Used when the key conditions trail a run of assignment bindings (update),
    /// so `$n` numbering stays contiguous with the preceding binds.
    pub(crate) fn where_sql_from(&self, offset: usize) -> String {
        self.columns
            .iter()
            .enumerate()
            .map(|(index, column)| format!("{column} = ${}", offset + index + 1))
            .collect::<Vec<_>>()
            .join(" AND ")
    }

    /// A stable string id for an activity event.
    ///
    /// A single-key model keeps its bare value (a UUID renders as its canonical
    /// text, matching the pre-composite event shape); a composite key joins its
    /// values with `,`.
    pub(crate) fn event_id(&self) -> String {
        let parts: Vec<String> = self.values.iter().map(render_bare).collect();
        parts.join(",")
    }
}

/// Render a bind value as a bare string for an activity event id.
fn render_bare(value: &Value) -> String {
    match value {
        Value::Uuid(id) => id.to_string(),
        Value::Text(text) => text.clone(),
        Value::Int(int) => int.to_string(),
        Value::Float(float) => float.to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Null => String::new(),
        other => other.to_json().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    /// Verifies a single-key filter renders one equality clause and a bare id.
    #[test]
    fn single_filter_renders_one_clause() {
        let id = Uuid::nil();
        let filter = KeyFilter {
            columns: vec!["id"],
            values: vec![Value::Uuid(id)],
        };
        assert_eq!(filter.where_sql(), "id = $1");
        assert_eq!(filter.event_id(), id.to_string());
    }

    /// Verifies a composite filter renders a conjunction in column order and
    /// honors the placeholder offset.
    #[test]
    fn composite_filter_renders_conjunction() {
        let filter = KeyFilter {
            columns: vec!["tenant_id", "user_id"],
            values: vec![Value::Text("t1".into()), Value::Text("u1".into())],
        };
        assert_eq!(filter.where_sql(), "tenant_id = $1 AND user_id = $2");
        assert_eq!(filter.where_sql_from(3), "tenant_id = $4 AND user_id = $5");
        assert_eq!(filter.event_id(), "t1,u1");
    }
}
