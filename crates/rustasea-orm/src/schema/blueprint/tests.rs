//! Unit tests for the fluent [`Blueprint`] column builder.

use super::*;
use crate::error::OrmError;

/// Verifies a full blueprint emits SQLite DDL with inline increments and index.
#[test]
fn create_emits_sqlite_ddl() {
    let mut blueprint = Blueprint::create("users");
    blueprint.id();
    blueprint.string("email", 255).unique();
    blueprint.string("name", 255).nullable();
    blueprint.timestamps();
    let sql = blueprint.to_sql("sqlite").unwrap();
    assert!(
        sql.starts_with("CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, "),
        "{sql}"
    );
    assert!(sql.contains("email VARCHAR(255) NOT NULL"), "{sql}");
    assert!(
        sql.contains("CREATE UNIQUE INDEX users_email_unique ON users (email)"),
        "{sql}"
    );
    assert!(sql.ends_with(';'), "{sql}");
}

/// Verifies the same blueprint renders MySQL with an InnoDB suffix.
#[test]
fn create_emits_mysql_suffix() {
    let mut blueprint = Blueprint::create("users");
    blueprint.id();
    let sql = blueprint.to_sql("mysql").unwrap();
    assert!(
        sql.starts_with(
            "CREATE TABLE users (id BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY) ENGINE=InnoDB"
        ),
        "{sql}"
    );
}

/// Verifies a composite primary key renders as a table constraint.
#[test]
fn composite_primary_key() {
    let mut blueprint = Blueprint::create("roles");
    blueprint.integer("user_id");
    blueprint.integer("role_id");
    blueprint.primary(["user_id", "role_id"]);
    let sql = blueprint.to_sql("postgres").unwrap();
    assert!(sql.contains("PRIMARY KEY (user_id, role_id)"), "{sql}");
}

/// Verifies alter mode emits ADD/DROP COLUMN and CREATE INDEX.
#[test]
fn alter_emits_statements() {
    let mut blueprint = Blueprint::alter("users");
    blueprint.drop_column("legacy");
    blueprint.string("nickname", 100).nullable();
    blueprint.index(["nickname"]);
    let sql = blueprint.to_sql("sqlite").unwrap();
    assert!(
        sql.contains("ALTER TABLE users DROP COLUMN legacy"),
        "{sql}"
    );
    assert!(
        sql.contains("ALTER TABLE users ADD COLUMN nickname VARCHAR(100)"),
        "{sql}"
    );
    assert!(
        sql.contains("CREATE INDEX users_nickname_index ON users (nickname)"),
        "{sql}"
    );
}

/// Verifies an empty blueprint and duplicate columns are typed errors.
#[test]
fn rejects_empty_and_duplicate() {
    let empty = Blueprint::create("t");
    assert!(matches!(
        empty.to_sql("sqlite"),
        Err(OrmError::Schema(SchemaError::EmptyBlueprint { .. }))
    ));

    let mut dup = Blueprint::create("t");
    dup.string("a", 10);
    dup.string("a", 20);
    assert!(matches!(
        dup.to_sql("sqlite"),
        Err(OrmError::Schema(SchemaError::DuplicateColumn { .. }))
    ));
}

/// Verifies an illegal table identifier is rejected before emission.
#[test]
fn rejects_illegal_table_name() {
    let mut blueprint = Blueprint::create("bad table");
    blueprint.id();
    assert!(matches!(
        blueprint.to_sql("sqlite"),
        Err(OrmError::Schema(SchemaError::InvalidIdentifier { .. }))
    ));
}

/// Verifies table-level index/unique columns are validated and non-empty.
#[test]
fn rejects_bad_and_empty_index_columns() {
    let mut injection = Blueprint::create("users");
    injection.string("email", 120);
    injection.index(["email); DROP TABLE users; --"]);
    assert!(matches!(
        injection.to_sql("sqlite"),
        Err(OrmError::Schema(SchemaError::InvalidIdentifier { .. }))
    ));

    let mut unique_injection = Blueprint::create("users");
    unique_injection.string("email", 120);
    unique_injection.unique(["email", "bad name"]);
    assert!(matches!(
        unique_injection.to_sql("sqlite"),
        Err(OrmError::Schema(SchemaError::InvalidIdentifier { .. }))
    ));

    let mut empty = Blueprint::create("users");
    empty.string("email", 120);
    empty.index([] as [&str; 0]);
    assert!(matches!(
        empty.to_sql("sqlite"),
        Err(OrmError::Schema(SchemaError::EmptyIndexColumns))
    ));
}

/// Verifies `json_index` emits a dialect-shaped expression index per driver.
#[test]
fn json_index_emits_expression_index() {
    let mut blueprint = Blueprint::create("posts");
    blueprint.text("payload");
    blueprint.json_index("payload", "author.id");

    let sqlite = blueprint.to_sql("sqlite").unwrap();
    assert!(
        sqlite.contains(
            "CREATE INDEX posts_payload_0_json ON posts (json_extract(payload, '$.author.id'))"
        ),
        "{sqlite}"
    );

    let postgres = blueprint.to_sql("postgres").unwrap();
    assert!(
        postgres.contains("CREATE INDEX posts_payload_0_json ON posts ((payload ->> 'author.id'))"),
        "{postgres}"
    );

    let mysql = blueprint.to_sql("mysql").unwrap();
    assert!(
        mysql.contains(
            "CREATE INDEX posts_payload_0_json ON posts ((CAST(JSON_UNQUOTE(JSON_EXTRACT(payload, '$.author.id')) AS CHAR(255))))"
        ),
        "{mysql}"
    );
}

/// Verifies a `json_index` with a blank column/path is a typed error.
#[test]
fn json_index_rejects_blank_parts() {
    let mut blueprint = Blueprint::create("posts");
    blueprint.text("payload");
    blueprint.json_index("payload", "   ");
    assert!(matches!(
        blueprint.to_sql("sqlite"),
        Err(OrmError::Schema(SchemaError::EmptyColumnName))
    ));
}
