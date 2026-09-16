//! Dialect-aware DDL emission for the schema builder.
//!
//! The active dialect is supplied at emission time as a runtime `&str` — the
//! same convention used by [`crate::model_ops`] and [`crate::builder::ext`] — so
//! a blueprint authored once renders valid `CREATE TABLE`/`ALTER TABLE` SQL for
//! SQLite, Postgres, and MySQL without recompiling. Unknown dialects surface as
//! [`SchemaError::UnknownDialect`] before any SQL is produced.

use super::column::{Column, ColumnKind, DefaultValue};
use crate::error::{Result, SchemaError};

/// Canonical database dialect resolved from a runtime string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dialect {
    /// SQLite.
    Sqlite,
    /// PostgreSQL.
    Postgres,
    /// MySQL / MariaDB.
    MySql,
}

impl Dialect {
    /// Resolve a runtime dialect string, accepting the common aliases.
    ///
    /// Accepted: `sqlite`; `postgres`/`postgresql`/`pg`; `mysql`/`mariadb`.
    pub(crate) fn resolve(dialect: &str) -> Result<Self> {
        match dialect {
            "sqlite" => Ok(Dialect::Sqlite),
            "postgres" | "postgresql" | "pg" => Ok(Dialect::Postgres),
            "mysql" | "mariadb" => Ok(Dialect::MySql),
            other => Err(SchemaError::UnknownDialect(other.to_string()).into()),
        }
    }
}

/// Validate a table/column identifier, returning a typed error when illegal.
///
/// An identifier must start with an ASCII letter or underscore and contain only
/// ASCII letters, digits, or underscores. `empty_error` is returned for an empty
/// (or whitespace-only) name so callers can distinguish table vs column context.
pub(crate) fn validate_identifier(name: &str, empty_error: SchemaError) -> Result<()> {
    if name.trim().is_empty() {
        return Err(empty_error.into());
    }
    let mut chars = name.chars();
    let starts_ok = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_');
    let rest_ok = chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if starts_ok && rest_ok {
        Ok(())
    } else {
        Err(SchemaError::InvalidIdentifier {
            identifier: name.to_string(),
        }
        .into())
    }
}

/// The dialect-specific SQL type (plus inline key clauses) for a column kind.
pub(crate) fn type_sql(kind: &ColumnKind, dialect: Dialect) -> String {
    match kind {
        ColumnKind::Increments => match dialect {
            Dialect::Sqlite => "INTEGER PRIMARY KEY AUTOINCREMENT".to_string(),
            Dialect::Postgres => "BIGSERIAL PRIMARY KEY".to_string(),
            Dialect::MySql => "BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY".to_string(),
        },
        ColumnKind::String(len) => format!("VARCHAR({len})"),
        ColumnKind::Text => "TEXT".to_string(),
        ColumnKind::Integer => match dialect {
            Dialect::MySql => "INT".to_string(),
            _ => "INTEGER".to_string(),
        },
        ColumnKind::BigInteger => match dialect {
            Dialect::Sqlite => "INTEGER".to_string(),
            _ => "BIGINT".to_string(),
        },
        ColumnKind::Boolean => match dialect {
            Dialect::Sqlite => "INTEGER".to_string(),
            Dialect::Postgres => "BOOLEAN".to_string(),
            Dialect::MySql => "TINYINT(1)".to_string(),
        },
        ColumnKind::Timestamp => match dialect {
            Dialect::Sqlite => "TEXT".to_string(),
            Dialect::Postgres => "TIMESTAMPTZ".to_string(),
            Dialect::MySql => "TIMESTAMP".to_string(),
        },
        ColumnKind::Decimal { precision, scale } => match dialect {
            Dialect::MySql => format!("DECIMAL({precision},{scale})"),
            _ => format!("NUMERIC({precision},{scale})"),
        },
        ColumnKind::Json => match dialect {
            Dialect::Sqlite => "TEXT".to_string(),
            Dialect::Postgres => "JSONB".to_string(),
            Dialect::MySql => "JSON".to_string(),
        },
        ColumnKind::Uuid => match dialect {
            Dialect::Sqlite => "TEXT".to_string(),
            Dialect::Postgres => "UUID".to_string(),
            Dialect::MySql => "CHAR(36)".to_string(),
        },
        ColumnKind::ForeignId => match dialect {
            Dialect::Sqlite => "INTEGER".to_string(),
            Dialect::Postgres => "BIGINT".to_string(),
            Dialect::MySql => "BIGINT UNSIGNED".to_string(),
        },
    }
}

/// Render a [`DefaultValue`] as a dialect-specific SQL literal.
pub(crate) fn default_sql(value: &DefaultValue, dialect: Dialect) -> String {
    match value {
        DefaultValue::Null => "NULL".to_string(),
        DefaultValue::Bool(flag) => match dialect {
            Dialect::Postgres => if *flag { "TRUE" } else { "FALSE" }.to_string(),
            _ => if *flag { "1" } else { "0" }.to_string(),
        },
        DefaultValue::Int(int) => int.to_string(),
        DefaultValue::Float(float) => format!("{float}"),
        DefaultValue::Text(text) => format!("'{}'", text.replace('\'', "''")),
        DefaultValue::Raw(raw) => raw.clone(),
    }
}

/// Render one column definition for a `CREATE TABLE` body or `ADD COLUMN`.
pub(crate) fn column_sql(column: &Column, dialect: Dialect) -> String {
    let mut parts = vec![column.name.clone(), type_sql(&column.kind, dialect)];
    let increments = column.kind == ColumnKind::Increments;
    if !increments && !column.nullable {
        parts.push("NOT NULL".to_string());
    }
    if let Some(default) = &column.default {
        parts.push(format!("DEFAULT {}", default_sql(default, dialect)));
    }
    parts.join(" ")
}

/// Build a `CREATE [UNIQUE] INDEX` statement for `columns` on `table`.
pub(crate) fn index_statement(table: &str, columns: &[String], unique: bool) -> String {
    let kind = if unique { "unique" } else { "index" };
    let name = format!("{}_{}_{}", table, columns.join("_"), kind);
    let prefix = if unique {
        "CREATE UNIQUE INDEX"
    } else {
        "CREATE INDEX"
    };
    format!("{prefix} {name} ON {table} ({})", columns.join(", "))
}

/// Build a dialect-shaped JSON expression index on `column` at `path`.
///
/// `ordinal` disambiguates several JSON indexes on the same table/column so
/// their names never collide. The path is escaped into the SQL string literal
/// (single quotes doubled) so it cannot break out of the expression.
pub(crate) fn json_index_statement(
    table: &str,
    column: &str,
    path: &str,
    ordinal: usize,
    dialect: Dialect,
) -> String {
    let name = format!("{table}_{column}_{ordinal}_json");
    let escaped = path.replace('\'', "''");
    let expression = match dialect {
        Dialect::Sqlite => format!("json_extract({column}, '$.{escaped}')"),
        Dialect::Postgres => format!("({column} ->> '{escaped}')"),
        Dialect::MySql => {
            format!("(CAST(JSON_UNQUOTE(JSON_EXTRACT({column}, '$.{escaped}')) AS CHAR(255)))")
        }
    };
    format!("CREATE INDEX {name} ON {table} ({expression})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::OrmError;

    /// Verifies canonical names and aliases resolve to the expected dialect.
    #[test]
    fn resolves_dialect_aliases() {
        assert_eq!(Dialect::resolve("sqlite").unwrap(), Dialect::Sqlite);
        assert_eq!(Dialect::resolve("postgres").unwrap(), Dialect::Postgres);
        assert_eq!(Dialect::resolve("pg").unwrap(), Dialect::Postgres);
        assert_eq!(Dialect::resolve("mariadb").unwrap(), Dialect::MySql);
    }

    /// Verifies an unknown dialect is a typed error.
    #[test]
    fn unknown_dialect_errors() {
        assert!(matches!(
            Dialect::resolve("oracle"),
            Err(OrmError::Schema(SchemaError::UnknownDialect(name))) if name == "oracle"
        ));
    }

    /// Verifies identifier validation accepts legal names and rejects illegal ones.
    #[test]
    fn identifier_validation() {
        assert!(validate_identifier("user_id", SchemaError::EmptyColumnName).is_ok());
        assert!(validate_identifier("_hidden", SchemaError::EmptyColumnName).is_ok());
        assert!(matches!(
            validate_identifier("", SchemaError::EmptyColumnName),
            Err(OrmError::Schema(SchemaError::EmptyColumnName))
        ));
        assert!(matches!(
            validate_identifier("2fast", SchemaError::EmptyColumnName),
            Err(OrmError::Schema(SchemaError::InvalidIdentifier { .. }))
        ));
        assert!(matches!(
            validate_identifier("bad name", SchemaError::EmptyColumnName),
            Err(OrmError::Schema(SchemaError::InvalidIdentifier { .. }))
        ));
    }

    /// Verifies boolean defaults differ between Postgres and the others.
    #[test]
    fn boolean_default_is_dialect_specific() {
        assert_eq!(
            default_sql(&DefaultValue::Bool(true), Dialect::Postgres),
            "TRUE"
        );
        assert_eq!(default_sql(&DefaultValue::Bool(true), Dialect::Sqlite), "1");
        assert_eq!(default_sql(&DefaultValue::Bool(false), Dialect::MySql), "0");
    }

    /// Verifies text defaults escape single quotes.
    #[test]
    fn text_default_escapes_quotes() {
        assert_eq!(
            default_sql(&DefaultValue::Text("O'Brien".into()), Dialect::Sqlite),
            "'O''Brien'"
        );
    }
}
