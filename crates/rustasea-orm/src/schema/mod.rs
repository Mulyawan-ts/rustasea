//! Dialect-agnostic schema builder — Laravel `Schema`/`Blueprint` parity.
//!
//! [`Schema`] is the entry point for authoring DDL. Each method builds a
//! [`Blueprint`], collects column definitions and indexes, and renders a single
//! `;`-separated SQL script for the requested **runtime** dialect:
//!
//! ```rust,ignore
//! use rustasea_orm::schema::Schema;
//!
//! let sql = Schema::create("users", |table| {
//!     table.id();
//!     table.string("email", 255).unique();
//!     table.string("name", 255).nullable();
//!     table.foreign_id("team_id").index();
//!     table.boolean("active").default(true);
//!     table.timestamps();
//! })?;
//! // sql is valid DDL for the dialect you pass to Schema::create's blueprint.
//! ```
//!
//! The dialect is supplied at emission time as a `&str` (`"sqlite"`,
//! `"postgres"`/`"postgresql"`/`"pg"`, or `"mysql"`/`"mariadb"`) — the same
//! runtime-dialect convention used by [`crate::model_ops`] and
//! [`crate::builder::ext`]. One blueprint therefore renders valid DDL for every
//! supported driver without recompiling:
//!
//! ```rust,ignore
//! let blueprint = Schema::blueprint("users"); // build once…
//! let sqlite   = blueprint.to_sql("sqlite")?;
//! let postgres = blueprint.to_sql("postgres")?;
//! let mysql    = blueprint.to_sql("mysql")?;
//! ```
//!
//! # Migration integration
//!
//! [`Migration::up`](crate::migration::Migration::up) returns a SQL `String`, so
//! a migration body may simply return the output of [`Schema::create`] (or
//! [`Schema::drop`] for its reverse). The [`Migration`](crate::migration::Migration)
//! trait signature is unchanged:
//!
//! ```rust,ignore
//! use rustasea_orm::migration::Migration;
//! use rustasea_orm::schema::Schema;
//!
//! struct CreateUsers;
//!
//! impl Migration for CreateUsers {
//!     fn name(&self) -> &str { "0001_create_users_table" }
//!
//!     fn up(&self) -> rustasea_orm::Result<String> {
//!         Schema::create("users", |table| {
//!             table.id();
//!             table.string("email", 255).unique();
//!         })
//!     }
//!
//!     fn down(&self) -> rustasea_orm::Result<String> {
//!         Schema::drop_if_exists("users")
//!     }
//! }
//! ```
//!
//! The dialect a migration renders for is chosen by the author (usually by
//! threading the live pool's [`DbPool::dialect`](crate::db::DbPool::dialect)
//! into the emission call), so the emitted script tracks the driver actually in
//! use rather than the compiled feature set.

mod blueprint;
mod column;
mod emit;

pub use blueprint::Blueprint;
pub use column::{Column, ColumnKind, DefaultValue};

/// Crate-root alias of [`Blueprint`].
///
/// The crate already re-exports a vector-helper [`crate::blueprint::Blueprint`]
/// under the bare `Blueprint` name, so the schema builder's blueprint is
/// surfaced at the root as `SchemaBlueprint` to avoid a collision. Inside this
/// module the type remains [`Blueprint`].
pub use blueprint::Blueprint as SchemaBlueprint;

use crate::error::Result;
use emit::Dialect;

/// Entry point for dialect-agnostic schema authoring.
///
/// All methods validate their inputs and return a typed
/// [`SchemaError`](crate::error::SchemaError) before producing any SQL, so a
/// successful call always yields an executable statement for the target
/// dialect. See the [module docs](self) for the migration-integration pattern.
pub struct Schema;

impl Schema {
    /// Build a [`Blueprint`] for `table` without committing to a mode.
    ///
    /// Prefer [`Schema::create`] / [`Schema::table`]; this escape hatch is useful
    /// when the same blueprint must be rendered for several dialects (call
    /// [`Blueprint::to_sql`] once per dialect).
    pub fn blueprint(table: &str) -> Blueprint {
        Blueprint::create(table)
    }

    /// Compile a `CREATE TABLE` script for `table` using the runtime default
    /// dialect ([`crate::builder::dialect`]).
    ///
    /// `configure` receives a mutable [`Blueprint`] and registers the columns and
    /// indexes. For an explicit dialect use [`Schema::create_for`], or build the
    /// blueprint once with [`Schema::blueprint`] and call [`Blueprint::to_sql`]
    /// per dialect.
    ///
    /// ```rust,ignore
    /// let sql = Schema::create("posts", |table| {
    ///     table.id();
    ///     table.string("title", 200);
    ///     table.text("body").nullable();
    /// })?;
    /// ```
    pub fn create<F>(table: &str, configure: F) -> Result<String>
    where
        F: FnOnce(&mut Blueprint),
    {
        Self::create_for(table, crate::builder::dialect(), configure)
    }

    /// Compile a `CREATE TABLE` script for `table` targeting an explicit runtime
    /// `dialect`.
    ///
    /// See [`Schema::create`] for the dialect-agnostic form.
    pub fn create_for<F>(table: &str, dialect: &str, configure: F) -> Result<String>
    where
        F: FnOnce(&mut Blueprint),
    {
        let mut blueprint = Blueprint::create(table);
        configure(&mut blueprint);
        blueprint.to_sql(dialect)
    }

    /// Compile an `ALTER TABLE` script for `table` (add/drop column, add index)
    /// using the runtime default dialect.
    ///
    /// ```rust,ignore
    /// let sql = Schema::table("users", |table| {
    ///     table.string("nickname", 100).nullable();
    ///     table.drop_column("legacy");
    ///     table.index(["nickname"]);
    /// })?;
    /// ```
    pub fn table<F>(table: &str, configure: F) -> Result<String>
    where
        F: FnOnce(&mut Blueprint),
    {
        Self::table_for(table, crate::builder::dialect(), configure)
    }

    /// Compile an `ALTER TABLE` script for `table` targeting an explicit runtime
    /// `dialect`.
    ///
    /// See [`Schema::table`] for the dialect-agnostic form.
    pub fn table_for<F>(table: &str, dialect: &str, configure: F) -> Result<String>
    where
        F: FnOnce(&mut Blueprint),
    {
        let mut blueprint = Blueprint::alter(table);
        configure(&mut blueprint);
        blueprint.to_sql(dialect)
    }

    /// Compile a `DROP TABLE <table>` statement.
    pub fn drop(table: &str) -> Result<String> {
        emit::validate_identifier(table, crate::error::SchemaError::EmptyTableName)?;
        Ok(format!("DROP TABLE {table};"))
    }

    /// Compile a `DROP TABLE IF EXISTS <table>` statement.
    pub fn drop_if_exists(table: &str) -> Result<String> {
        emit::validate_identifier(table, crate::error::SchemaError::EmptyTableName)?;
        Ok(format!("DROP TABLE IF EXISTS {table};"))
    }

    /// Resolve a runtime dialect string, exposing the canonical name.
    ///
    /// Returns [`SchemaError::UnknownDialect`](crate::error::SchemaError::UnknownDialect)
    /// for anything other than the accepted aliases.
    pub fn resolve_dialect(dialect: &str) -> Result<&'static str> {
        Ok(match Dialect::resolve(dialect)? {
            Dialect::Sqlite => "sqlite",
            Dialect::Postgres => "postgres",
            Dialect::MySql => "mysql",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{OrmError, SchemaError};

    /// Verifies `Schema::create` emits a complete SQLite script.
    #[test]
    fn create_emits_script() {
        let sql = Schema::create_for("users", "sqlite", |table| {
            table.id();
            table.string("email", 255).unique();
        })
        .unwrap();
        assert!(sql.starts_with("CREATE TABLE users ("), "{sql}");
        assert!(sql.ends_with(';'), "{sql}");
    }

    /// Verifies the dialect-agnostic `create`/`table` forms use the runtime dialect.
    #[test]
    fn create_and_table_use_runtime_dialect() {
        let create = Schema::create("widgets", |table| {
            table.id();
        })
        .unwrap();
        assert!(create.starts_with("CREATE TABLE widgets ("), "{create}");

        let alter = Schema::table("widgets", |table| {
            table.string("label", 50).nullable();
        })
        .unwrap();
        assert!(
            alter.contains("ALTER TABLE widgets ADD COLUMN label"),
            "{alter}"
        );
    }

    /// Verifies `drop` and `drop_if_exists` render the expected statements.
    #[test]
    fn drop_statements() {
        assert_eq!(Schema::drop("users").unwrap(), "DROP TABLE users;");
        assert_eq!(
            Schema::drop_if_exists("users").unwrap(),
            "DROP TABLE IF EXISTS users;"
        );
    }

    /// Verifies drop rejects an empty/illegal table name.
    #[test]
    fn drop_rejects_empty_table() {
        assert!(matches!(
            Schema::drop(""),
            Err(OrmError::Schema(SchemaError::EmptyTableName))
        ));
        assert!(matches!(
            Schema::drop("bad-name"),
            Err(OrmError::Schema(SchemaError::InvalidIdentifier { .. }))
        ));
    }

    /// Verifies an unknown dialect is a typed error.
    #[test]
    fn unknown_dialect_is_typed() {
        let error = Schema::create_for("users", "oracle", |table| {
            table.id();
        })
        .unwrap_err();
        assert!(
            matches!(error, OrmError::Schema(SchemaError::UnknownDialect(_))),
            "{error:?}"
        );
    }

    /// Verifies the same blueprint renders each dialect distinctly.
    #[test]
    fn same_blueprint_all_dialects() {
        let mut blueprint = Schema::blueprint("events");
        blueprint.id();
        blueprint.json("payload").nullable();
        blueprint.timestamp("at").nullable();

        let sqlite = blueprint.to_sql("sqlite").unwrap();
        let postgres = blueprint.to_sql("postgres").unwrap();
        let mysql = blueprint.to_sql("mysql").unwrap();

        assert!(
            sqlite.contains("id INTEGER PRIMARY KEY AUTOINCREMENT"),
            "{sqlite}"
        );
        assert!(postgres.contains("id BIGSERIAL PRIMARY KEY"), "{postgres}");
        assert!(
            mysql.contains("id BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY"),
            "{mysql}"
        );
        assert!(sqlite.contains("payload TEXT"), "{sqlite}");
        assert!(postgres.contains("payload JSONB"), "{postgres}");
        assert!(mysql.contains("payload JSON"), "{mysql}");
        assert!(sqlite.contains("at TEXT"), "{sqlite}");
        assert!(postgres.contains("at TIMESTAMPTZ"), "{postgres}");
        assert!(mysql.contains("at TIMESTAMP"), "{mysql}");
    }

    /// Verifies `resolve_dialect` normalises aliases and rejects unknown names.
    #[test]
    fn resolves_dialect_aliases() {
        assert_eq!(Schema::resolve_dialect("pg").unwrap(), "postgres");
        assert_eq!(Schema::resolve_dialect("mariadb").unwrap(), "mysql");
        assert!(matches!(
            Schema::resolve_dialect("nope"),
            Err(OrmError::Schema(SchemaError::UnknownDialect(_)))
        ));
    }
}
