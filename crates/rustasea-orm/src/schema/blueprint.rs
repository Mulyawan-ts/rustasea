//! The [`Blueprint`] fluent column builder.
//!
//! A blueprint accumulates columns plus table-level indexes and keys, then
//! renders dialect-specific DDL through [`Blueprint::to_sql`]. It backs both
//! `CREATE TABLE` ([`Schema::create`](super::Schema::create)) and `ALTER TABLE`
//! ([`Schema::table`](super::Schema::table)) flows.
//!
//! Column helpers return a mutable handle to the column they just added so the
//! Laravel-style modifier chain reads naturally:
//!
//! ```rust,ignore
//! let sql = Schema::create("users", |table| {
//!     table.id();
//!     table.string("email", 255).unique();
//!     table.string("name", 255).nullable();
//!     table.timestamps();
//! })?;
//! ```
//!
//! The dialect is a runtime argument to [`Blueprint::to_sql`], so the same
//! blueprint renders valid SQLite, Postgres, and MySQL DDL.

use super::column::{Column, ColumnKind};
use super::emit::{self, Dialect};
use crate::error::{Result, SchemaError};

/// Whether a blueprint creates a new table or alters an existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// `CREATE TABLE …`.
    Create,
    /// `ALTER TABLE …` — add/drop columns and add indexes.
    Alter,
}

/// Fluent builder for a single table's DDL.
///
/// Obtain one through [`Schema::create`](super::Schema::create) or
/// [`Schema::table`](super::Schema::table); do not construct it directly.
#[derive(Debug, Clone)]
pub struct Blueprint {
    /// Target table name.
    table: String,
    /// Create vs alter behaviour.
    mode: Mode,
    /// Columns to add (all columns in `Create` mode).
    columns: Vec<Column>,
    /// Column names to drop (alter mode only).
    drops: Vec<String>,
    /// Table-level `PRIMARY KEY` columns (composite keys).
    primary: Vec<String>,
    /// Table-level unique indexes.
    unique_indexes: Vec<Vec<String>>,
    /// Table-level plain indexes.
    indexes: Vec<Vec<String>>,
    /// JSON expression indexes: `(column, path)` pairs (ADOPT-020).
    json_indexes: Vec<(String, String)>,
}

impl Blueprint {
    /// Start a `CREATE TABLE` blueprint for `table`.
    pub(crate) fn create(table: &str) -> Self {
        Self::new(table, Mode::Create)
    }

    /// Start an `ALTER TABLE` blueprint for `table`.
    pub(crate) fn alter(table: &str) -> Self {
        Self::new(table, Mode::Alter)
    }

    /// Construct an empty blueprint in `mode`.
    fn new(table: &str, mode: Mode) -> Self {
        Self {
            table: table.to_string(),
            mode,
            columns: Vec::new(),
            drops: Vec::new(),
            primary: Vec::new(),
            unique_indexes: Vec::new(),
            indexes: Vec::new(),
            json_indexes: Vec::new(),
        }
    }

    /// The target table name.
    pub fn table_name(&self) -> &str {
        &self.table
    }

    /// Add a column of `kind` named `name` and return a handle to it.
    fn push(&mut self, name: &str, kind: ColumnKind) -> &mut Column {
        self.columns.push(Column::new(name, kind));
        self.columns.last_mut().expect("just pushed a column")
    }

    /// Add an auto-incrementing `id` primary key.
    pub fn id(&mut self) -> &mut Column {
        self.columns.push(Column::increments("id"));
        self.columns.last_mut().expect("just pushed a column")
    }

    /// Add an auto-incrementing primary key named `name`.
    pub fn big_increments(&mut self, name: &str) -> &mut Column {
        self.columns.push(Column::increments(name));
        self.columns.last_mut().expect("just pushed a column")
    }

    /// Add a variable-length string column with maximum `len`.
    pub fn string(&mut self, name: &str, len: u32) -> &mut Column {
        self.push(name, ColumnKind::String(len))
    }

    /// Add an unbounded text column.
    pub fn text(&mut self, name: &str) -> &mut Column {
        self.push(name, ColumnKind::Text)
    }

    /// Add a 32-bit integer column.
    pub fn integer(&mut self, name: &str) -> &mut Column {
        self.push(name, ColumnKind::Integer)
    }

    /// Add a 64-bit integer column.
    pub fn big_integer(&mut self, name: &str) -> &mut Column {
        self.push(name, ColumnKind::BigInteger)
    }

    /// Add a boolean column.
    pub fn boolean(&mut self, name: &str) -> &mut Column {
        self.push(name, ColumnKind::Boolean)
    }

    /// Add a timestamp-with-time-zone column.
    pub fn timestamp(&mut self, name: &str) -> &mut Column {
        self.push(name, ColumnKind::Timestamp)
    }

    /// Add nullable `created_at` and `updated_at` timestamp columns.
    pub fn timestamps(&mut self) -> &mut Self {
        self.push("created_at", ColumnKind::Timestamp).nullable();
        self.push("updated_at", ColumnKind::Timestamp).nullable();
        self
    }

    /// Add a fixed-precision decimal column.
    pub fn decimal(&mut self, name: &str, precision: u32, scale: u32) -> &mut Column {
        self.push(name, ColumnKind::Decimal { precision, scale })
    }

    /// Add a JSON column.
    pub fn json(&mut self, name: &str) -> &mut Column {
        self.push(name, ColumnKind::Json)
    }

    /// Add a UUID column.
    pub fn uuid(&mut self, name: &str) -> &mut Column {
        self.push(name, ColumnKind::Uuid)
    }

    /// Add an unsigned 64-bit foreign-key column.
    pub fn foreign_id(&mut self, name: &str) -> &mut Column {
        self.push(name, ColumnKind::ForeignId)
    }

    /// Drop a column (alter mode only).
    pub fn drop_column(&mut self, name: &str) -> &mut Self {
        self.drops.push(name.to_string());
        self
    }

    /// Declare a table-level `PRIMARY KEY` over `columns`.
    pub fn primary<I, S>(&mut self, columns: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.primary
            .extend(columns.into_iter().map(|c| c.as_ref().to_string()));
        self
    }

    /// Declare a table-level unique index over `columns`.
    ///
    /// Columns are validated when the blueprint is rendered by
    /// [`Blueprint::to_sql`]: an illegal identifier yields
    /// [`SchemaError::InvalidIdentifier`] and an empty list yields
    /// [`SchemaError::EmptyIndexColumns`].
    pub fn unique<I, S>(&mut self, columns: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.unique_indexes.push(names(columns));
        self
    }

    /// Declare a table-level (non-unique) index over `columns`.
    ///
    /// Columns are validated when the blueprint is rendered by
    /// [`Blueprint::to_sql`]: an illegal identifier yields
    /// [`SchemaError::InvalidIdentifier`] and an empty list yields
    /// [`SchemaError::EmptyIndexColumns`].
    pub fn index<I, S>(&mut self, columns: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.indexes.push(names(columns));
        self
    }

    /// Declare a JSON expression index on `column` at `path` (ADOPT-020).
    ///
    /// Renders a dialect-shaped expression index so a `*_json` relation
    /// predicate can use it:
    ///
    /// * SQLite: `json_extract(column, '$.path')`
    /// * Postgres: `(column ->> 'path')`
    /// * MySQL: `(CAST(JSON_UNQUOTE(JSON_EXTRACT(column, '$.path')) AS CHAR(255)))`
    ///
    /// The column and path are validated when the blueprint is rendered; a blank
    /// column or path yields [`SchemaError::EmptyColumnName`].
    pub fn json_index(&mut self, column: &str, path: &str) -> &mut Self {
        self.json_indexes
            .push((column.to_string(), path.to_string()));
        self
    }

    /// Render the blueprint as dialect-specific DDL.
    ///
    /// Validates the dialect, table name, column names, and blueprint shape,
    /// returning a typed [`SchemaError`] before any SQL is produced. Statements
    /// are joined with `";\n"` and terminated with a trailing `";"`.
    pub fn to_sql(&self, dialect: &str) -> Result<String> {
        let dialect = Dialect::resolve(dialect)?;
        emit::validate_identifier(&self.table, SchemaError::EmptyTableName)?;

        let mut statements: Vec<String> = Vec::new();
        match self.mode {
            Mode::Create => self.emit_create(dialect, &mut statements)?,
            Mode::Alter => self.emit_alter(dialect, &mut statements)?,
        }
        Ok(statements.join(";\n") + ";")
    }

    /// Validate column names for uniqueness and legality.
    fn validated_columns(&self) -> Result<()> {
        let mut seen: Vec<&str> = Vec::with_capacity(self.columns.len());
        for column in &self.columns {
            emit::validate_identifier(&column.name, SchemaError::EmptyColumnName)?;
            if seen.contains(&column.name.as_str()) {
                return Err(SchemaError::DuplicateColumn {
                    table: self.table.clone(),
                    column: column.name.clone(),
                }
                .into());
            }
            seen.push(&column.name);
        }
        Ok(())
    }

    /// Emit `CREATE TABLE` plus any index statements.
    fn emit_create(&self, dialect: Dialect, out: &mut Vec<String>) -> Result<()> {
        if self.columns.is_empty() {
            return Err(SchemaError::EmptyBlueprint {
                table: self.table.clone(),
            }
            .into());
        }
        self.validated_columns()?;

        let mut definitions: Vec<String> = self
            .columns
            .iter()
            .map(|column| emit::column_sql(column, dialect))
            .collect();
        if !self.primary.is_empty() {
            for name in &self.primary {
                emit::validate_identifier(name, SchemaError::EmptyColumnName)?;
            }
            definitions.push(format!("PRIMARY KEY ({})", self.primary.join(", ")));
        }

        let suffix = match dialect {
            Dialect::MySql => " ENGINE=InnoDB",
            _ => "",
        };
        out.push(format!(
            "CREATE TABLE {} ({}){suffix}",
            self.table,
            definitions.join(", ")
        ));
        self.emit_indexes(dialect, out)?;
        Ok(())
    }

    /// Emit `ALTER TABLE` statements for added/dropped columns and indexes.
    fn emit_alter(&self, dialect: Dialect, out: &mut Vec<String>) -> Result<()> {
        if self.columns.is_empty()
            && self.drops.is_empty()
            && self.indexes.is_empty()
            && self.unique_indexes.is_empty()
            && self.json_indexes.is_empty()
        {
            return Err(SchemaError::EmptyBlueprint {
                table: self.table.clone(),
            }
            .into());
        }
        self.validated_columns()?;
        for name in &self.drops {
            emit::validate_identifier(name, SchemaError::EmptyColumnName)?;
            out.push(format!("ALTER TABLE {} DROP COLUMN {}", self.table, name));
        }
        for column in &self.columns {
            let definition = emit::column_sql(column, dialect);
            out.push(format!(
                "ALTER TABLE {} ADD COLUMN {}",
                self.table, definition
            ));
        }
        self.emit_indexes(dialect, out)?;
        Ok(())
    }

    /// Emit column-level and table-level index statements.
    ///
    /// Every index column identifier is validated against the same allow-list
    /// used for table and column names before any statement is produced, so a
    /// malicious name (e.g. `"col); DROP TABLE users; --"`) surfaces as a typed
    /// [`SchemaError::InvalidIdentifier`] rather than reaching the SQL string.
    /// A table-level `index`/`unique` declaration with no columns is rejected
    /// with [`SchemaError::EmptyIndexColumns`].
    fn emit_indexes(&self, dialect: Dialect, out: &mut Vec<String>) -> Result<()> {
        for column in &self.columns {
            if column.is_unique() {
                out.push(emit::index_statement(
                    &self.table,
                    std::slice::from_ref(&column.name),
                    true,
                ));
            } else if column.is_indexed() {
                out.push(emit::index_statement(
                    &self.table,
                    std::slice::from_ref(&column.name),
                    false,
                ));
            }
        }
        for columns in &self.unique_indexes {
            self.validate_index_columns(columns)?;
            out.push(emit::index_statement(&self.table, columns, true));
        }
        for columns in &self.indexes {
            self.validate_index_columns(columns)?;
            out.push(emit::index_statement(&self.table, columns, false));
        }
        self.emit_json_indexes(dialect, out)?;
        Ok(())
    }

    /// Emit JSON expression indexes declared via [`Blueprint::json_index`].
    ///
    /// Each column is validated as an identifier (the path is escaped into the
    /// SQL literal) before the dialect-shaped expression index is produced.
    fn emit_json_indexes(&self, dialect: Dialect, out: &mut Vec<String>) -> Result<()> {
        for (index, (column, path)) in self.json_indexes.iter().enumerate() {
            emit::validate_identifier(column, SchemaError::EmptyColumnName)?;
            if path.trim().is_empty() {
                return Err(SchemaError::EmptyColumnName.into());
            }
            out.push(emit::json_index_statement(
                &self.table,
                column,
                path,
                index,
                dialect,
            ));
        }
        Ok(())
    }

    /// Validate a table-level index column list before it is emitted.
    ///
    /// Rejects an empty list with [`SchemaError::EmptyIndexColumns`] and any
    /// non-conforming identifier with [`SchemaError::InvalidIdentifier`].
    fn validate_index_columns(&self, columns: &[String]) -> Result<()> {
        if columns.is_empty() {
            return Err(SchemaError::EmptyIndexColumns.into());
        }
        for name in columns {
            emit::validate_identifier(name, SchemaError::EmptyColumnName)?;
        }
        Ok(())
    }
}

/// Collect an iterator of column names into a `Vec<String>`.
fn names<I, S>(columns: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    columns
        .into_iter()
        .map(|c| c.as_ref().to_string())
        .collect()
}

#[cfg(test)]
mod tests;
