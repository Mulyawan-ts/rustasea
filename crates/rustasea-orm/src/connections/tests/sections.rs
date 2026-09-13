//! Migrations and Redis section parsing (Laravel `database.php` parity).
//!
//! Covers the two top-level sections added alongside `connections`: the
//! `[database.migrations]` tracking-table settings and the `[database.redis]`
//! connection-definition map. Positive cases parse a full file; absent sections
//! fall back to defaults; malformed values surface typed errors.

use super::*;

/// A config exercising migrations plus the `default`/`cache` Redis connections.
const MIGRATIONS_AND_REDIS: &str = r#"
[database]
default = "sqlite"

[database.connections.sqlite]
driver = "sqlite"
url = "sqlite://database.sqlite?mode=rwc"

[database.migrations]
table = "app_migrations"
update_date_on_publish = false

[database.redis]
client = "deadpool"

[database.redis.options]
cluster = "redis"
prefix = "rustasea-database-"
persistent = false

[database.redis.default]
url = "redis://127.0.0.1:6379/0"
host = "127.0.0.1"
port = 6379
database = 0
max_retries = 3
backoff_algorithm = "exponential"
backoff_base = 100
backoff_cap = 1000

[database.redis.cache]
url = "redis://127.0.0.1:6379/1"
host = "127.0.0.1"
port = 6379
database = 1
max_retries = 5
backoff_algorithm = "exponential"
backoff_base = 200
backoff_cap = 2000
"#;

/// Parse a `database.toml` body through the layered loader.
fn parse(contents: &str) -> Result<DatabaseConfig> {
    let dir = TempConfigDir::new();
    dir.write_database(contents);
    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load config");
    DatabaseConfig::from_loader(&loader)
}

/// The migrations section parses into the typed tracking-table settings.
#[test]
fn parses_migrations_section() {
    let config = parse(MIGRATIONS_AND_REDIS).expect("parse");
    assert_eq!(config.migrations_table(), "app_migrations");
    assert_eq!(config.migrations.table, "app_migrations");
    assert!(!config.update_date_on_publish());
    assert!(!config.migrations.update_date_on_publish);
}

/// The Redis section exposes its client, options, and named connections.
#[test]
fn parses_redis_section() {
    let config = parse(MIGRATIONS_AND_REDIS).expect("parse");
    let redis = config.redis.as_ref().expect("redis section");
    assert_eq!(redis.client(), "deadpool");
    assert_eq!(redis.options.cluster, "redis");
    assert_eq!(redis.options.prefix, "rustasea-database-");
    assert!(!redis.options.persistent);
    assert_eq!(redis.connections.len(), 2);

    let default = config.redis_connection("default").expect("default redis");
    assert_eq!(default.database, Some(0));
    assert_eq!(default.port, Some(6379));
    assert_eq!(default.max_retries, Some(3));
    assert_eq!(default.backoff_base, Some(100));
    assert_eq!(default.backoff_cap, Some(1000));
    assert_eq!(default.backoff_algorithm.as_deref(), Some("exponential"));

    let cache = config.redis_connection("cache").expect("cache redis");
    assert_eq!(cache.database, Some(1));
    assert_eq!(cache.max_retries, Some(5));
    assert_eq!(cache.backoff_base, Some(200));
    assert_eq!(cache.backoff_cap, Some(2000));
}

/// An absent migrations section yields the Laravel defaults.
#[test]
fn absent_migrations_section_defaults() {
    let config = load_three();
    assert_eq!(config.migrations_table(), "migrations");
    assert!(config.update_date_on_publish());
    assert!(config.redis.is_none());
    assert!(config.redis_connection("default").is_none());
}

/// A `[database.migrations]` table without `table` keeps the default name.
#[test]
fn migrations_table_defaults_when_omitted() {
    let config = parse("[database.migrations]\nupdate_date_on_publish = false\n").expect("parse");
    assert_eq!(config.migrations_table(), "migrations");
    assert!(!config.update_date_on_publish());
}

/// A Redis URL with an unsupported scheme is a typed error.
#[test]
fn malformed_redis_url_is_typed_error() {
    let error = parse(
        "[database.redis]\nclient = \"deadpool\"\n\n\
         [database.redis.default]\nurl = \"http://127.0.0.1:6379\"\n",
    )
    .expect_err("bad scheme must fail");
    assert!(
        matches!(error, OrmError::Connection(ConnectionError::InvalidConfig(ref detail)) if detail.contains("default") && detail.contains("http")),
        "got {error:?}"
    );
}

/// A non-numeric Redis port is a typed deserialization error, not a panic.
#[test]
fn malformed_redis_port_is_typed_error() {
    let error = parse(
        "[database.redis]\nclient = \"deadpool\"\n\n\
         [database.redis.default]\nport = \"not-a-number\"\n",
    )
    .expect_err("bad port must fail");
    assert!(
        matches!(
            error,
            OrmError::Connection(ConnectionError::InvalidConfig(_))
        ),
        "got {error:?}"
    );
}

/// A valid `rediss://` (TLS) URL is accepted by validation.
#[test]
fn redis_tls_scheme_is_accepted() {
    let config = parse(
        "[database.redis]\nclient = \"deadpool\"\n\n\
         [database.redis.default]\nurl = \"rediss://127.0.0.1:6380/0\"\n",
    )
    .expect("rediss scheme is valid");
    assert_eq!(
        config
            .redis_connection("default")
            .and_then(|c| c.url.as_deref()),
        Some("rediss://127.0.0.1:6380/0")
    );
}
