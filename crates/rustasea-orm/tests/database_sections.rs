//! End-to-end coverage for the `[database.migrations]` / `[database.redis]`
//! sections and the configurable migration tracking table.
//!
//! Two contracts are proven here:
//!
//! 1. the shipped `config/database.toml` parses, exposing the migrations table
//!    and the `default`/`cache` Redis connection definitions; and
//! 2. [`Migrator::with_migrations_table`] records into a custom tracking table
//!    (never the default `migrations` table), so a configured name is honoured
//!    without breaking the default behaviour.

use rustasea_config::ConfigLoader;
use rustasea_orm::connections::DatabaseConfig;
use rustasea_orm::{DbPool, Migration, Migrator, Result as OrmResult, Value};
use std::path::PathBuf;

/// A throwaway directory holding a copied `database.toml`.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Create a fresh process-unique temp directory.
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("rustasea-orm-sections-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    /// The directory path.
    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempDir {
    /// Remove the temp directory and its contents.
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Load the shipped `config/database.toml` through the layered loader.
fn load_shipped() -> DatabaseConfig {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let shipped = manifest.join("../../config/database.toml");
    let contents = std::fs::read_to_string(&shipped).expect("read shipped database.toml");
    let dir = TempDir::new();
    std::fs::write(dir.path().join("database.toml"), contents).expect("write copy");
    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load config");
    DatabaseConfig::from_loader(&loader).expect("parse shipped database.toml")
}

/// The shipped file parses and exposes the migrations tracking-table settings.
#[test]
fn shipped_config_exposes_migrations_section() {
    let config = load_shipped();
    assert_eq!(config.migrations_table(), "migrations");
    assert!(config.update_date_on_publish());
}

/// The shipped file exposes the `default` and `cache` Redis connections.
#[test]
fn shipped_config_exposes_redis_connections() {
    let config = load_shipped();
    let redis = config.redis.as_ref().expect("redis section");
    assert_eq!(redis.client(), "deadpool");
    assert_eq!(redis.options.prefix, "rustasea-database-");

    let default = config.redis_connection("default").expect("default");
    assert_eq!(default.database, Some(0));
    assert_eq!(default.port, Some(6379));
    assert_eq!(default.max_retries, Some(3));
    assert_eq!(default.backoff_base, Some(100));
    assert_eq!(default.backoff_cap, Some(1000));

    let cache = config.redis_connection("cache").expect("cache");
    assert_eq!(cache.database, Some(1));
    assert_eq!(cache.host.as_deref(), Some("127.0.0.1"));
    assert_eq!(cache.backoff_algorithm.as_deref(), Some("exponential"));
}

/// A migration creating one table.
struct CreateWidgets;

impl Migration for CreateWidgets {
    fn name(&self) -> &str {
        "0001_create_widgets_table"
    }

    fn up(&self) -> OrmResult<String> {
        Ok("CREATE TABLE widgets (id INTEGER PRIMARY KEY);".into())
    }

    fn down(&self) -> OrmResult<String> {
        Ok("DROP TABLE widgets;".into())
    }
}

/// Whether `table` exists in the SQLite schema.
async fn table_exists(pool: &DbPool, table: &str) -> bool {
    let rows = pool
        .fetch_json(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = $1",
            &[Value::Text(table.to_string())],
        )
        .await
        .expect("schema query");
    !rows.is_empty()
}

/// A custom tracking table is created and used; the default is never touched.
#[tokio::test]
async fn custom_migrations_table_is_honoured() {
    let pool = DbPool::connect("sqlite::memory:").await.expect("pool");
    let mut migrator = Migrator::new();
    migrator.with_migrations_table("app_migrations");
    migrator.add(CreateWidgets);

    let applied = migrator.run(&pool).await.expect("run");
    assert_eq!(applied, vec!["0001_create_widgets_table".to_string()]);

    assert!(
        table_exists(&pool, "app_migrations").await,
        "custom tracking table must be created"
    );
    assert!(
        !table_exists(&pool, "migrations").await,
        "default tracking table must not be created"
    );

    // A second run is a no-op against the custom table.
    let second = migrator.run(&pool).await.expect("idempotent");
    assert!(second.is_empty());

    // Rollback deletes the custom row and reverts the schema.
    let rolled_back = migrator.rollback(&pool).await.expect("rollback");
    assert_eq!(rolled_back, vec!["0001_create_widgets_table".to_string()]);
    assert!(!table_exists(&pool, "widgets").await);

    let recorded = pool
        .fetch_json("SELECT name FROM app_migrations", &[])
        .await
        .expect("tracking query");
    assert!(
        recorded.is_empty(),
        "rollback clears the custom tracking row"
    );
}
