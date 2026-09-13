//! `[queue]` config parity + driver registration (CFG-004).
//!
//! Positive: a full Laravel-shaped `queue.toml` parses into [`QueueConfig`],
//! `sync` + `database` drivers register, and the default connection comes from
//! config. Negative: an unknown driver, an unknown default connection, and a
//! `database` connection missing its `table` each surface a typed
//! [`QueueConfigError`].

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use rustasea_config::ConfigLoader;
use rustasea_queue::{QueueConfig, QueueConfigError, QueueRegistry};

/// Serializes tests that read or mutate process-global `QUEUE_*` variables.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A unique temporary directory removed when dropped.
struct TempConfigDir {
    path: PathBuf,
}

impl TempConfigDir {
    /// Create an empty temp directory with a process-unique name.
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rustasea-queue-config-{}-{}",
            std::process::id(),
            unique
        ));
        std::fs::create_dir_all(&path).expect("create temp config dir");
        Self { path }
    }

    /// Write `contents` to `name` inside the temp directory.
    fn write(&self, name: &str, contents: &str) {
        std::fs::write(self.path.join(name), contents).expect("write temp config file");
    }

    /// Borrow the temp directory path.
    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempConfigDir {
    /// Remove the temp directory and all of its contents.
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// The full Laravel 13.x `queue.php` shape mapped onto TOML.
const FULL_CONFIG: &str = r#"
[queue]
default = "database"

[queue.connections.sync]
driver = "sync"

[queue.connections.database]
driver = "database"
connection = "sqlite"
table = "jobs"
queue = "default"
retry_after = 90
after_commit = false

[queue.connections.redis]
driver = "redis"
connection = "default"
queue = "default"
retry_after = 90
block_for = 5

[queue.batching]
database = "sqlite"
table = "job_batches"

[queue.failed]
driver = "database-uuids"
database = "sqlite"
table = "failed_jobs"
"#;

/// Parse a config from an inline TOML body via a temp `queue.toml`.
fn parse(toml: &str) -> Result<QueueConfig, QueueConfigError> {
    let dir = TempConfigDir::new();
    dir.write("queue.toml", toml);
    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load config");
    QueueConfig::from_loader(&loader)
}

/// Positive: the full Laravel shape parses into every typed block.
#[test]
fn parses_full_laravel_queue_shape() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let config = parse(FULL_CONFIG).expect("valid config");

    assert_eq!(config.default, "database");
    assert_eq!(config.connections.len(), 3);

    let sync = config.connection("sync").expect("sync declared");
    assert!(sync.is_sync());
    assert_eq!(sync.queue, "default");

    let database = config.connection("database").expect("database declared");
    assert!(database.is_database());
    assert_eq!(database.connection.as_deref(), Some("sqlite"));
    assert_eq!(database.table.as_deref(), Some("jobs"));
    assert_eq!(database.retry_after, 90);
    assert!(!database.after_commit);

    let redis = config.connection("redis").expect("redis declared");
    assert!(redis.is_redis());
    assert_eq!(redis.block_for, Some(5));

    assert_eq!(config.batching.table, "job_batches");
    assert_eq!(config.batching.database.as_deref(), Some("sqlite"));
    assert_eq!(config.failed.driver, "database-uuids");
    assert_eq!(config.failed_table(), "failed_jobs");
    assert_eq!(config.batches_table(), "job_batches");
}

/// Positive: `sync` + `database` register and the default comes from config.
#[tokio::test]
async fn registers_sync_and_database_and_sets_default() {
    let config = {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        parse(FULL_CONFIG).expect("valid config")
    };
    let pool = rustasea_orm::DbPool::connect("sqlite::memory:")
        .await
        .expect("in-memory pool");

    QueueRegistry::configure_from(&config, Some(pool)).expect("configure");

    assert!(QueueRegistry::default_connection() == "database");
    assert!(rustasea_queue::Queue::driver("sync").is_ok());
    assert!(rustasea_queue::Queue::driver("database").is_ok());
}

/// Negative: an unknown driver names the offending connection and driver.
#[test]
fn unknown_driver_is_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let error = parse(
        r#"
[queue]
default = "sqs"
[queue.connections.sqs]
driver = "sqs"
queue = "default"
"#,
    )
    .expect_err("sqs is unimplemented");
    assert_eq!(
        error,
        QueueConfigError::UnsupportedDriver {
            connection: "sqs".to_string(),
            driver: "sqs".to_string(),
        }
    );
}

/// Negative: `default` naming an undeclared connection is a typed error.
#[test]
fn unknown_default_connection_is_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let error = parse(
        r#"
[queue]
default = "missing"
[queue.connections.sync]
driver = "sync"
"#,
    )
    .expect_err("default must resolve");
    assert_eq!(
        error,
        QueueConfigError::UnknownConnection {
            name: "missing".to_string(),
        }
    );
}

/// Negative: a `database` connection without a `table` is a typed error.
#[test]
fn missing_table_is_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let error = parse(
        r#"
[queue]
default = "database"
[queue.connections.database]
driver = "database"
queue = "default"
"#,
    )
    .expect_err("table is required");
    assert_eq!(
        error,
        QueueConfigError::MissingField {
            connection: "database".to_string(),
            field: "table".to_string(),
        }
    );
}

/// Positive: `QUEUE_CONNECTION` overrides the file's `queue.default`.
#[test]
fn queue_connection_env_overrides_file() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("QUEUE_CONNECTION", "sync");

    let config = parse(FULL_CONFIG).expect("valid config");

    std::env::remove_var("QUEUE_CONNECTION");
    assert_eq!(config.default, "sync");
}

/// Positive: a blank `QUEUE_CONNECTION` leaves the file's default intact.
#[test]
fn queue_connection_blank_env_is_ignored() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("QUEUE_CONNECTION", "   ");

    let config = parse(FULL_CONFIG).expect("valid config");

    std::env::remove_var("QUEUE_CONNECTION");
    assert_eq!(config.default, "database");
}
