//! Fluent SQL query builder (in-memory SQL emission; sqlx execution wired in S03-T01).

use crate::error::{OrmError, Result};
use crate::scopes::GlobalScopeEntry;
use crate::types::{JsonFilter, Value};
use std::any::TypeId;

pub use crate::clause::{Lock, OrderDirection, Raw, SqlFragment};

mod exec;
mod ext;
mod global;

pub(crate) use exec::json_to_model;
pub use exec::Executor;

mod composite;
mod eager;

#[cfg(test)]
mod tests;

/// Driver dialect selected via cargo features (Postgres default).
pub fn dialect() -> &'static str {
    #[cfg(feature = "postgres")]
    {
        "postgres"
    }
    #[cfg(all(not(feature = "postgres"), feature = "mysql"))]
    {
        "mysql"
    }
    #[cfg(all(not(feature = "postgres"), not(feature = "mysql"), feature = "sqlite"))]
    {
        "sqlite"
    }
    #[cfg(not(any(feature = "postgres", feature = "mysql", feature = "sqlite")))]
    {
        "unknown"
    }
}

/// A `JOIN` clause carried by the builder (`straight_join` hint).
#[derive(Debug, Clone)]
struct JoinClause {
    sql: String,
}

/// A single WHERE condition.
#[derive(Clone)]
struct Condition {
    glue: &'static str, // "AND" | "OR"
    sql: String,
    /// Per-condition bind values (reserved for named-scope composition in S03-T03).
    #[allow(dead_code)]
    bindings: Vec<Value>,
    /// Owning global scope, when the condition was added by one. Lets
    /// `without_global_scope::<S>` remove exactly that scope's clauses.
    scope_type: Option<TypeId>,
}

impl Condition {
    /// Build a bare condition (no bind values, no owning scope).
    fn new(glue: &'static str, sql: impl Into<String>) -> Self {
        Self {
            glue,
            sql: sql.into(),
            bindings: Vec::new(),
            scope_type: None,
        }
    }
}

/// Render an ordered run of conditions into a single SQL predicate.
///
/// The first condition's `glue` is ignored (nothing precedes it); every later
/// condition contributes `<glue> <sql>`.
fn render_conditions(conditions: &[&Condition]) -> String {
    let mut out = String::new();
    for (i, condition) in conditions.iter().enumerate() {
        if i > 0 {
            out.push(' ');
            out.push_str(condition.glue);
            out.push(' ');
        }
        out.push_str(&condition.sql);
    }
    out
}

/// A column ordering.
#[derive(Clone)]
struct OrderBy {
    column: String,
    direction: OrderDirection,
}

/// Fluent query builder — builds parameterized SQL without touching the DB.
#[derive(Default, Clone)]
pub struct QueryBuilder {
    table: String,
    columns: Vec<String>,
    conditions: Vec<Condition>,
    joins: Vec<JoinClause>,
    orders: Vec<OrderBy>,
    limit: Option<u64>,
    offset: Option<u64>,
    lock: Option<Lock>,
    /// Runtime lock dialect (`lock_with_dialect`); `None` = compile-time [`dialect`].
    lock_dialect: Option<String>,
    scope_active: Vec<String>,
    bindings: Vec<Value>,
    /// Soft-delete guard state: `None` = not applied, `true` = include trashed,
    /// `false` = active rows only (`deleted_at IS NULL`).
    soft_delete_guard: Option<bool>,
    /// Global scopes attached to this query; applied when the SQL is rendered.
    global_scopes: Vec<GlobalScopeEntry>,
    /// Whether `without_global_scopes()` disabled every global scope.
    global_scopes_bypassed: bool,
    /// Global scope types explicitly bypassed via `without_global_scope::<S>`.
    bypassed_scopes: Vec<TypeId>,
    /// Guard so a resolved builder is not re-resolved (double-application).
    global_scopes_applied: bool,
    /// Global scope currently applying, if any — conditions it pushes are tagged
    /// with its [`TypeId`].
    current_scope: Option<TypeId>,
    /// Relation names requested for eager loading via `with(&[...])`.
    eager: Vec<String>,
    /// Declared relation metadata used to resolve `eager` names.
    eager_declared: Vec<crate::model::Relation>,
}

impl QueryBuilder {
    /// The table this query targets.
    pub fn table_name(&self) -> &str {
        &self.table
    }

    /// Start a query against `table`.
    pub fn table(table: impl Into<String>) -> Self {
        Self {
            table: table.into(),
            ..Default::default()
        }
    }

    /// Select specific columns (default `*`).
    pub fn select(mut self, columns: &[&str]) -> Self {
        self.columns = columns.iter().map(|c| (*c).to_string()).collect();
        self
    }

    /// Push a WHERE condition, tagging it with the global scope currently
    /// applying (if any).
    fn push_condition(&mut self, glue: &'static str, sql: impl Into<String>) {
        let mut condition = Condition::new(glue, sql);
        condition.scope_type = self.current_scope;
        self.conditions.push(condition);
    }

    /// Add an equality WHERE clause: `where("email", value)`.
    pub fn where_eq(mut self, column: &str, value: impl Into<Value>) -> Self {
        let value = value.into();
        self.bindings.push(value);
        let idx = self.bindings.len();
        self.push_condition("AND", format!("{column} = ${idx}"));
        self
    }

    /// Add an OR equality WHERE clause.
    pub fn or_where_eq(mut self, column: &str, value: impl Into<Value>) -> Self {
        let value = value.into();
        self.bindings.push(value);
        let idx = self.bindings.len();
        self.push_condition("OR", format!("{column} = ${idx}"));
        self
    }

    /// Add an OR equality clause on the primary key (`orWhereKey`).
    pub fn or_where_key(self, column: &str, value: impl Into<Value>) -> Self {
        self.or_where_eq(column, value)
    }

    /// Add a `WHERE column = ?` on a raw byte string (`whereBinary`).
    pub fn where_binary(mut self, column: &str, value: &[u8]) -> Self {
        self.bindings.push(Value::Text(
            value.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        ));
        let idx = self.bindings.len();
        self.push_condition("AND", format!("encode({column}, 'hex') = ${idx}"));
        self
    }

    /// Add an arbitrary operator clause: `where("age", ">", 18)`.
    pub fn where_op(
        mut self,
        column: &str,
        operator: &str,
        value: impl Into<Value>,
    ) -> Result<Self> {
        let allowed = ["=", "!=", "<>", ">", ">=", "<", "<=", "LIKE", "NOT LIKE"];
        if !allowed.contains(&operator) {
            return Err(OrmError::InvalidState(format!(
                "unsupported operator `{operator}`"
            )));
        }
        let value = value.into();
        self.bindings.push(value);
        let idx = self.bindings.len();
        self.push_condition("AND", format!("{column} {operator} ${idx}"));
        Ok(self)
    }

    /// Splice a raw SQL fragment into the WHERE clause (unparameterized).
    pub fn where_raw(mut self, fragment: impl Into<SqlFragment>) -> Self {
        self.push_condition("AND", fragment.into().sql);
        self
    }

    /// Add a `WHERE column IN (...)` clause.
    pub fn where_in(mut self, column: &str, values: Vec<Value>) -> Self {
        let placeholders: Vec<String> = values
            .iter()
            .map(|v| {
                self.bindings.push(v.clone());
                format!("${}", self.bindings.len())
            })
            .collect();
        if placeholders.is_empty() {
            self.push_condition("AND", "1 = 0");
            return self;
        }
        self.push_condition("AND", format!("{column} IN ({})", placeholders.join(", ")));
        self
    }

    /// Add a `WHERE column IS NULL` clause (soft-delete guard uses this).
    pub fn where_null(mut self, column: &str) -> Self {
        self.push_condition("AND", format!("{column} IS NULL"));
        self
    }

    /// Add a JSON filter clause (`@>`, path equality, or key exists).
    ///
    /// Filters that compare against a value (`Contains`, `PathEquals`) bind that
    /// value; `KeyExists` needs no placeholder.
    pub fn where_json(mut self, column: &str, filter: JsonFilter) -> Result<Self> {
        let placeholder = filter.bind_value().map(|value| {
            self.bindings.push(value);
            format!("${}", self.bindings.len())
        });
        let fragment = filter.to_sql(column, dialect())?;
        let sql = match placeholder {
            Some(placeholder) => fragment.replace("{}", &placeholder),
            None => fragment,
        };
        self.push_condition("AND", sql);
        Ok(self)
    }

    /// Add a `WHERE id = $n` clause (findOrFail / whereKey).
    pub fn where_key(self, id: uuid::Uuid) -> Self {
        self.where_eq("id", Value::Uuid(id))
    }

    /// Order results by a column.
    pub fn order_by(mut self, column: &str, direction: OrderDirection) -> Self {
        self.orders.push(OrderBy {
            column: column.to_string(),
            direction,
        });
        self
    }

    /// Limit returned rows.
    pub fn limit(mut self, n: u64) -> Self {
        self.limit = Some(n);
        self
    }

    /// Skip rows (OFFSET pagination).
    pub fn offset(mut self, n: u64) -> Self {
        self.offset = Some(n);
        self
    }

    /// Bind values in positional order for this unresolved builder.
    ///
    /// **This reflects only the bindings the builder owns directly.** A global
    /// scope that binds values (e.g. a tenancy scope) contributes them to the
    /// resolved builder instead, so [`QueryBuilder::to_sql`] may emit more `$n`
    /// placeholders than `bindings()` returns. Callers pairing `to_sql()` with
    /// `bindings()` manually must use [`QueryBuilder::resolved_bindings`] (or
    /// `resolved().bindings()`) to stay aligned. The executor methods do this
    /// internally.
    pub fn bindings(&self) -> &[Value] {
        &self.bindings
    }

    /// Bind values for the resolved query, including scope-contributed values.
    ///
    /// Resolves global scopes exactly as [`QueryBuilder::to_sql`] does, so the
    /// returned values line up positionally with the `$n` placeholders in the
    /// rendered SQL. Use this whenever a query may carry global scopes; prefer
    /// `resolved().bindings()` when the resolved builder is reused.
    pub fn resolved_bindings(&self) -> Vec<Value> {
        self.resolved().bindings().to_vec()
    }

    /// Whether any rows would match (used by `first` semantics).
    pub fn has_limit(&self) -> bool {
        self.limit.is_some()
    }

    /// Whether the query currently filters on the primary key column.
    pub fn has_where_key(&self) -> bool {
        self.conditions
            .iter()
            .any(|c| c.sql.starts_with("id = $") || c.sql.starts_with("id IN ("))
    }

    /// Whether the builder has conditions applied.
    pub fn has_conditions(&self) -> bool {
        !self.conditions.is_empty()
    }

    /// The active ORDER BY clause (used by `delete_sql` for MySQL JOIN deletes).
    pub fn order_clause(&self) -> Option<String> {
        if self.orders.is_empty() {
            return None;
        }
        let parts: Vec<String> = self
            .orders
            .iter()
            .map(|o| format!("{} {}", o.column, o.direction.as_str()))
            .collect();
        Some(parts.join(", "))
    }

    /// Render the WHERE clause, or `None` when no conditions exist.
    ///
    /// Global scopes are applied to a copy before rendering, so callers see the
    /// scoped clause (e.g. `deleted_at IS NULL`) automatically.
    pub fn where_clause(&self) -> Option<String> {
        self.resolved().render_where_clause()
    }

    /// Render the raw WHERE clause of this exact builder (no scope resolution).
    ///
    /// Global-scope conditions are appended after caller conditions when the
    /// query resolves. If either run uses `OR`, that run is wrapped in
    /// parentheses before the two are joined with `AND`, so an appended
    /// `AND deleted_at IS NULL` binds to the whole caller predicate instead of
    /// being absorbed by the `OR` (SQL gives `AND` higher precedence).
    fn render_where_clause(&self) -> Option<String> {
        if self.conditions.is_empty() {
            return None;
        }
        let caller: Vec<&Condition> = self
            .conditions
            .iter()
            .filter(|condition| condition.scope_type.is_none())
            .collect();
        let scoped: Vec<&Condition> = self
            .conditions
            .iter()
            .filter(|condition| condition.scope_type.is_some())
            .collect();
        // Only the mix of caller and scope conditions needs precedence handling;
        // a homogeneous run is rendered in insertion order as before.
        if caller.is_empty() || scoped.is_empty() {
            return Some(render_conditions(
                &self.conditions.iter().collect::<Vec<_>>(),
            ));
        }
        let mut caller_sql = render_conditions(&caller);
        if caller
            .iter()
            .skip(1)
            .any(|condition| condition.glue == "OR")
        {
            caller_sql = format!("({caller_sql})");
        }
        let mut scoped_sql = render_conditions(&scoped);
        if scoped
            .iter()
            .skip(1)
            .any(|condition| condition.glue == "OR")
        {
            scoped_sql = format!("({scoped_sql})");
        }
        Some(format!("{caller_sql} AND {scoped_sql}"))
    }

    /// Render the SELECT statement with `$n` placeholders (never interpolates values).
    ///
    /// Global scopes are applied to a copy first, so a scoped model query emits
    /// its scope guards without any explicit call.
    pub fn to_sql(&self) -> Result<String> {
        self.resolved().render_sql()
    }

    /// Render this exact builder (no scope resolution) as a SELECT statement.
    fn render_sql(&self) -> Result<String> {
        let columns = if self.columns.is_empty() {
            "*".to_string()
        } else {
            self.columns.join(", ")
        };
        let mut sql = format!("SELECT {columns} FROM {}", self.table);
        for join in &self.joins {
            sql.push(' ');
            sql.push_str(&join.sql);
        }
        if let Some(clause) = self.render_where_clause() {
            sql.push_str(" WHERE ");
            sql.push_str(&clause);
        }
        if !self.orders.is_empty() {
            let parts: Vec<String> = self
                .orders
                .iter()
                .map(|o| format!("{} {}", o.column, o.direction.as_str()))
                .collect();
            sql.push_str(" ORDER BY ");
            sql.push_str(&parts.join(", "));
        }
        if let Some(limit) = self.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }
        if let Some(offset) = self.offset {
            sql.push_str(&format!(" OFFSET {offset}"));
        }
        if let Some(lock) = self.lock {
            sql.push(' ');
            let lock_dialect = self.lock_dialect.as_deref().unwrap_or(dialect());
            sql.push_str(&lock.to_sql(lock_dialect)?);
        }
        Ok(sql)
    }

    /// Render the raw-SQL form: the table fragment with filters applied.
    ///
    /// Display-only diagnostic for `raw_sql` helpers; the SELECT projection
    /// never matters for count/delete/update emission.
    pub fn to_raw_sql_clause(&self) -> Result<String> {
        self.resolved().render_raw_clause()
    }

    /// Render this exact builder (no scope resolution) as a table fragment.
    fn render_raw_clause(&self) -> Result<String> {
        let mut sql = String::from("FROM ");
        sql.push_str(&self.table);
        for join in &self.joins {
            sql.push(' ');
            sql.push_str(&join.sql);
        }
        if let Some(clause) = self.render_where_clause() {
            sql.push_str(" WHERE ");
            sql.push_str(&clause);
        }
        Ok(sql)
    }

    /// Render the statement with inlined literals — **display only, never executed**.
    pub fn to_raw_sql(&self) -> Result<String> {
        let resolved = self.resolved();
        let mut out = resolved.render_sql()?;
        for (i, value) in resolved.bindings.iter().enumerate() {
            out = out.replace(&format!("${}", i + 1), &value.to_literal());
        }
        Ok(out)
    }
}
