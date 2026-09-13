//! Integration tests for the dialect-agnostic schema builder.
//!
//! Covers the DB-003 contract: one [`SchemaBlueprint`] emits golden DDL for all
//! three dialects; `Schema::create` output executes against a live in-memory
//! SQLite pool (then INSERT/SELECT round-trips); unknown dialects, empty table
//! names, and illegal identifiers surface typed [`SchemaError`]s; and a
//! [`Migration`] may return `Schema::create` output without changing the trait.

use rustasea_orm::{
    DbPool, Migration, Migrator, OrmError, Result as OrmResult, Schema, SchemaBlueprint,
    SchemaError, Value,
};

/// A representative blueprint exercising the common column kinds and modifiers.
fn accounts_blueprint() -> SchemaBlueprint {
    let mut blueprint = Schema::blueprint("accounts");
    blueprint.id();
    blueprint.string("email", 120).unique();
    blueprint.boolean("active").default(true);
    blueprint.json("meta").nullable();
    blueprint.timestamp("created_at").nullable();
    blueprint
}

/// Golden DDL: the same blueprint renders valid SQLite SQL.
#[test]
fn golden_sqlite_ddl() {
    let sql = accounts_blueprint().to_sql("sqlite").unwrap();
    assert_eq!(
        sql,
        "CREATE TABLE accounts (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            email VARCHAR(120) NOT NULL, \
            active INTEGER NOT NULL DEFAULT 1, \
            meta TEXT, \
            created_at TEXT\
         );\n\
         CREATE UNIQUE INDEX accounts_email_unique ON accounts (email);"
    );
}

/// Golden DDL: the same blueprint renders valid Postgres SQL.
#[test]
fn golden_postgres_ddl() {
    let sql = accounts_blueprint().to_sql("postgres").unwrap();
    assert_eq!(
        sql,
        "CREATE TABLE accounts (\
            id BIGSERIAL PRIMARY KEY, \
            email VARCHAR(120) NOT NULL, \
            active BOOLEAN NOT NULL DEFAULT TRUE, \
            meta JSONB, \
            created_at TIMESTAMPTZ\
         );\n\
         CREATE UNIQUE INDEX accounts_email_unique ON accounts (email);"
    );
}

/// Golden DDL: the same blueprint renders valid MySQL SQL with an InnoDB suffix.
#[test]
fn golden_mysql_ddl() {
    let sql = accounts_blueprint().to_sql("mysql").unwrap();
    assert_eq!(
        sql,
        "CREATE TABLE accounts (\
            id BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY, \
            email VARCHAR(120) NOT NULL, \
            active TINYINT(1) NOT NULL DEFAULT 1, \
            meta JSON, \
            created_at TIMESTAMP\
         ) ENGINE=InnoDB;\n\
         CREATE UNIQUE INDEX accounts_email_unique ON accounts (email);"
    );
}

/// Verifies decimal precision/scale and the remaining column helpers render.
#[test]
fn decimal_and_remaining_columns() {
    let mut blueprint = Schema::blueprint("orders");
    blueprint.big_increments("order_id");
    blueprint.big_integer("customer_id");
    blueprint.integer("quantity");
    blueprint.decimal("total", 15, 2);
    blueprint.uuid("reference").unique();
    blueprint.foreign_id("user_id").index();
    blueprint.text("notes").nullable();

    let sqlite = blueprint.to_sql("sqlite").unwrap();
    assert!(
        sqlite.contains("order_id INTEGER PRIMARY KEY AUTOINCREMENT"),
        "{sqlite}"
    );
    assert!(sqlite.contains("customer_id INTEGER NOT NULL"), "{sqlite}");
    assert!(sqlite.contains("total NUMERIC(15,2) NOT NULL"), "{sqlite}");
    assert!(sqlite.contains("reference TEXT NOT NULL"), "{sqlite}");
    assert!(
        sqlite.contains("CREATE UNIQUE INDEX orders_reference_unique"),
        "{sqlite}"
    );
    assert!(
        sqlite.contains("CREATE INDEX orders_user_id_index"),
        "{sqlite}"
    );

    let postgres = blueprint.to_sql("postgres").unwrap();
    assert!(postgres.contains("reference UUID NOT NULL"), "{postgres}");

    let mysql = blueprint.to_sql("mysql").unwrap();
    assert!(mysql.contains("reference CHAR(36) NOT NULL"), "{mysql}");
    assert!(mysql.contains("total DECIMAL(15,2) NOT NULL"), "{mysql}");
}

/// Verifies the alter flow emits ADD/DROP COLUMN plus an index.
#[test]
fn alter_table_emits_statements() {
    let sql = Schema::table_for("accounts", "postgres", |table| {
        table.string("nickname", 80).nullable();
        table.drop_column("legacy");
        table.index(["nickname"]);
    })
    .unwrap();
    assert!(
        sql.contains("ALTER TABLE accounts ADD COLUMN nickname VARCHAR(80)"),
        "{sql}"
    );
    assert!(
        sql.contains("ALTER TABLE accounts DROP COLUMN legacy"),
        "{sql}"
    );
    assert!(
        sql.contains("CREATE INDEX accounts_nickname_index ON accounts (nickname)"),
        "{sql}"
    );
}

/// Verifies `drop` / `drop_if_exists` emit valid SQLite statements.
#[test]
fn drop_statements() {
    assert_eq!(Schema::drop("accounts").unwrap(), "DROP TABLE accounts;");
    assert_eq!(
        Schema::drop_if_exists("accounts").unwrap(),
        "DROP TABLE IF EXISTS accounts;"
    );
}

/// Verifies a `Schema::create` script executes on live SQLite, then INSERT/SELECT.
#[tokio::test]
async fn create_executes_and_round_trips_on_sqlite() {
    let pool = DbPool::connect("sqlite::memory:").await.unwrap();
    let ddl = Schema::create_for("accounts", "sqlite", |table| {
        table.id();
        table.string("email", 120).unique();
        table.boolean("active").default(true);
        table.timestamp("created_at").nullable();
    })
    .unwrap();
    pool.execute_script(&ddl).await.expect("DDL executes");

    pool.execute_bind(
        "INSERT INTO accounts (email, active) VALUES ($1, $2)",
        &[Value::Text("ada@example.com".into()), Value::Bool(true)],
    )
    .await
    .expect("insert");

    let rows = pool
        .fetch_json("SELECT id, email FROM accounts", &[])
        .await
        .expect("select");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"].as_i64(), Some(1));
    assert_eq!(rows[0]["email"].as_str(), Some("ada@example.com"));
}

/// Verifies the unique index emitted by the blueprint is enforced at runtime.
#[tokio::test]
async fn unique_index_is_enforced_on_sqlite() {
    let pool = DbPool::connect("sqlite::memory:").await.unwrap();
    let ddl = Schema::create_for("users", "sqlite", |table| {
        table.id();
        table.string("email", 120).unique();
    })
    .unwrap();
    pool.execute_script(&ddl).await.unwrap();

    pool.execute_bind(
        "INSERT INTO users (email) VALUES ($1)",
        &[Value::Text("dup@example.com".into())],
    )
    .await
    .unwrap();
    let second = pool
        .execute_bind(
            "INSERT INTO users (email) VALUES ($1)",
            &[Value::Text("dup@example.com".into())],
        )
        .await;
    assert!(
        second.is_err(),
        "unique index must reject a duplicate email"
    );
}

/// Negative: an unknown dialect is a typed `UnknownDialect` error.
#[test]
fn unknown_dialect_is_typed_error() {
    let error = Schema::create_for("accounts", "oracle", |table| {
        table.id();
    })
    .unwrap_err();
    assert!(
        matches!(error, OrmError::Schema(SchemaError::UnknownDialect(ref name)) if name == "oracle"),
        "{error:?}"
    );
}

/// Negative: an empty table name is a typed `EmptyTableName` error.
#[test]
fn empty_table_name_is_typed_error() {
    let error = Schema::drop_if_exists("   ").unwrap_err();
    assert!(
        matches!(error, OrmError::Schema(SchemaError::EmptyTableName)),
        "{error:?}"
    );
}

/// Negative: an illegal identifier is a typed `InvalidIdentifier` error.
#[test]
fn invalid_identifier_is_typed_error() {
    let error = Schema::create_for("bad table", "sqlite", |table| {
        table.id();
    })
    .unwrap_err();
    assert!(
        matches!(
            error,
            OrmError::Schema(SchemaError::InvalidIdentifier { .. })
        ),
        "{error:?}"
    );

    let column_error = Schema::create_for("ok", "sqlite", |table| {
        table.string("2fast", 10);
    })
    .unwrap_err();
    assert!(
        matches!(
            column_error,
            OrmError::Schema(SchemaError::InvalidIdentifier { .. })
        ),
        "{column_error:?}"
    );
}

/// Negative: an empty blueprint and duplicate columns are typed errors.
#[test]
fn empty_and_duplicate_blueprint_errors() {
    let empty = Schema::create_for("accounts", "sqlite", |_| {}).unwrap_err();
    assert!(
        matches!(empty, OrmError::Schema(SchemaError::EmptyBlueprint { .. })),
        "{empty:?}"
    );

    let dup = Schema::create_for("accounts", "sqlite", |table| {
        table.string("email", 10);
        table.string("email", 20);
    })
    .unwrap_err();
    assert!(
        matches!(dup, OrmError::Schema(SchemaError::DuplicateColumn { .. })),
        "{dup:?}"
    );
}

/// Negative: a malicious table-level index column is a typed error, not SQL.
#[test]
fn malicious_index_column_is_typed_error() {
    let injection = "col); DROP TABLE users; --";
    let index_error = Schema::create_for("accounts", "sqlite", |table| {
        table.string("email", 120);
        table.index([injection]);
    })
    .unwrap_err();
    assert!(
        matches!(
            index_error,
            OrmError::Schema(SchemaError::InvalidIdentifier { .. })
        ),
        "{index_error:?}"
    );

    let unique_error = Schema::create_for("accounts", "sqlite", |table| {
        table.string("email", 120);
        table.unique(["email", injection]);
    })
    .unwrap_err();
    assert!(
        matches!(
            unique_error,
            OrmError::Schema(SchemaError::InvalidIdentifier { .. })
        ),
        "{unique_error:?}"
    );
}

/// Negative: empty table-level index/unique column lists are typed errors.
#[test]
fn empty_index_columns_is_typed_error() {
    let index_error = Schema::create_for("accounts", "sqlite", |table| {
        table.string("email", 120);
        table.index([] as [&str; 0]);
    })
    .unwrap_err();
    assert!(
        matches!(
            index_error,
            OrmError::Schema(SchemaError::EmptyIndexColumns)
        ),
        "{index_error:?}"
    );

    let unique_error = Schema::table_for("accounts", "postgres", |table| {
        table.unique(Vec::<String>::new());
    })
    .unwrap_err();
    assert!(
        matches!(
            unique_error,
            OrmError::Schema(SchemaError::EmptyIndexColumns)
        ),
        "{unique_error:?}"
    );
}

/// Positive: a valid composite unique/index emits correct golden DDL per dialect.
#[test]
fn composite_index_golden_ddl_all_dialects() {
    fn build() -> SchemaBlueprint {
        let mut blueprint = Schema::blueprint("memberships");
        blueprint.big_increments("id");
        blueprint.big_integer("team_id");
        blueprint.big_integer("user_id");
        blueprint.string("role", 40);
        blueprint.unique(["team_id", "user_id"]);
        blueprint.index(["role", "team_id"]);
        blueprint
    }

    let sqlite = build().to_sql("sqlite").unwrap();
    assert!(
        sqlite.contains(
            "CREATE UNIQUE INDEX memberships_team_id_user_id_unique ON memberships (team_id, user_id)"
        ),
        "{sqlite}"
    );
    assert!(
        sqlite
            .contains("CREATE INDEX memberships_role_team_id_index ON memberships (role, team_id)"),
        "{sqlite}"
    );

    let postgres = build().to_sql("postgres").unwrap();
    assert!(
        postgres.contains(
            "CREATE UNIQUE INDEX memberships_team_id_user_id_unique ON memberships (team_id, user_id)"
        ),
        "{postgres}"
    );
    assert!(
        postgres
            .contains("CREATE INDEX memberships_role_team_id_index ON memberships (role, team_id)"),
        "{postgres}"
    );

    let mysql = build().to_sql("mysql").unwrap();
    assert!(
        mysql.contains(
            "CREATE UNIQUE INDEX memberships_team_id_user_id_unique ON memberships (team_id, user_id)"
        ),
        "{mysql}"
    );
    assert!(
        mysql
            .contains("CREATE INDEX memberships_role_team_id_index ON memberships (role, team_id)"),
        "{mysql}"
    );
}

/// Migration returning `Schema::create` output — the non-breaking integration path.
struct CreateAccounts;

impl Migration for CreateAccounts {
    fn name(&self) -> &str {
        "0001_create_accounts_table"
    }

    fn up(&self) -> OrmResult<String> {
        Schema::create_for("accounts", "sqlite", |table| {
            table.id();
            table.string("email", 120).unique();
            table.boolean("active").default(true);
        })
    }

    fn down(&self) -> OrmResult<String> {
        Schema::drop_if_exists("accounts")
    }
}

/// Verifies a `Migration` whose `up`/`down` return schema-builder output runs.
#[tokio::test]
async fn migration_can_return_schema_builder_output() {
    let pool = DbPool::connect("sqlite::memory:").await.unwrap();
    let mut migrator = Migrator::new();
    migrator.add(CreateAccounts);

    let applied = migrator.run(&pool).await.unwrap();
    assert_eq!(applied, vec!["0001_create_accounts_table".to_string()]);

    pool.execute_bind(
        "INSERT INTO accounts (email) VALUES ($1)",
        &[Value::Text("ada@example.com".into())],
    )
    .await
    .expect("table created by the migration accepts rows");

    let rolled_back = migrator.rollback(&pool).await.unwrap();
    assert_eq!(rolled_back, vec!["0001_create_accounts_table".to_string()]);
    assert!(pool
        .fetch_json("SELECT 1 FROM accounts", &[])
        .await
        .is_err());
}
