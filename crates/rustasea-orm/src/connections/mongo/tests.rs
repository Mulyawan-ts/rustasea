//! Tests for MongoDB integration in the named-connections resolver.
//!
//! Two layers are covered:
//!
//! * **Always available** — parsing a `driver = "mongodb"` connection, URI
//!   assembly from a `uri`/`url` or granular fields, and the typed
//!   [`ConnectionError::UnsupportedDriver`] returned by the SQL-only resolver.
//! * **Feature `mongodb`** — `resolve_any` returning
//!   [`DatabaseConnection::Mongo`] and [`rustasea_mongo::MongoConfig`]
//!   construction from a connection entry (env precedence preserved).

use super::*;
use crate::connections::DatabaseConfig;
use rustasea_config::ConfigLoader;

/// TOML with a MongoDB connection carrying an explicit `uri`.
const MONGO_URI_CONNECTION: &str = r#"
[database]
default = "sqlite"

[database.connections.sqlite]
driver = "sqlite"
url = "sqlite::memory:"

[database.connections.mongo]
driver = "mongodb"
uri = "mongodb://127.0.0.1:27017"
database = "rustasea"
"#;

/// TOML with a MongoDB connection built from granular fields.
const MONGO_GRANULAR_CONNECTION: &str = r#"
[database]
default = "mongo"

[database.connections.mongo]
driver = "mongodb"
host = "db.internal"
port = 27018
database = "app"
"#;

/// Parse a `database.toml` document through the layered loader.
fn parse(toml: &str) -> DatabaseConfig {
    let dir = std::env::temp_dir().join(format!(
        "rustasea-orm-mongo-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    std::fs::write(dir.join("database.toml"), toml).expect("write database.toml");
    let loader = ConfigLoader::load_from_dir(&dir).expect("load config");
    let config = DatabaseConfig::from_loader(&loader).expect("parse database config");
    let _ = std::fs::remove_dir_all(&dir);
    config
}

/// A `uri` key is accepted as an alias for `url` and parsed verbatim.
#[test]
fn parses_mongodb_connection_with_uri() {
    let config = parse(MONGO_URI_CONNECTION);
    let mongo = &config.connections["mongo"];
    assert!(mongo.is_mongo());
    assert_eq!(mongo.url.as_deref(), Some("mongodb://127.0.0.1:27017"));
    assert_eq!(mongo.mongo_database(), Some("rustasea"));
    assert_eq!(
        config.resolve_url(Some("mongo")).unwrap(),
        "mongodb://127.0.0.1:27017"
    );
}

/// `resolve_url(None)` follows the `default` selector to a MongoDB connection.
#[test]
fn default_selector_resolves_mongodb_uri() {
    let config = parse(MONGO_URI_CONNECTION);
    // Default is sqlite; switching it to mongo resolves the mongo URI.
    let mut config = config;
    config.default = Some("mongo".to_string());
    assert_eq!(
        config.resolve_url(None).unwrap(),
        "mongodb://127.0.0.1:27017"
    );
}

/// Granular fields build a valid `mongodb://host:port/database` URI.
#[test]
fn granular_fields_build_mongodb_uri() {
    let config = parse(MONGO_GRANULAR_CONNECTION);
    assert_eq!(
        config.resolve_url(Some("mongo")).unwrap(),
        "mongodb://db.internal:27018/app"
    );
}

/// A granular connection without a `host` is a typed missing-field error.
#[test]
fn mongodb_missing_host_is_typed_error() {
    let connection = ConnectionConfig {
        driver: "mongodb".into(),
        database: Some("app".into()),
        ..ConnectionConfig::default()
    };
    let error = connection.build_mongo_uri().unwrap_err();
    assert!(
        matches!(error, OrmError::Connection(ConnectionError::MissingField { ref field }) if field == "uri"),
        "got {error:?}"
    );
}

/// A non-MongoDB URL declared on a MongoDB connection is a typed mismatch.
#[test]
fn mongodb_driver_url_mismatch_is_typed_error() {
    let connection = ConnectionConfig {
        driver: "mongodb".into(),
        url: Some("postgres://host/db".into()),
        ..ConnectionConfig::default()
    };
    let error = connection.build_mongo_uri().unwrap_err();
    assert!(
        matches!(error, OrmError::Connection(ConnectionError::DriverMismatch { ref driver, ref scheme }) if driver == "mongodb" && scheme == "postgres"),
        "got {error:?}"
    );
}

/// A `mongodb+srv://` URI is accepted for an Atlas-style connection.
#[test]
fn mongodb_srv_uri_is_accepted() {
    let connection = ConnectionConfig {
        driver: "mongodb".into(),
        url: Some("mongodb+srv://cluster.example.net".into()),
        database: Some("rustasea".into()),
        ..ConnectionConfig::default()
    };
    assert_eq!(
        connection.build_mongo_uri().unwrap(),
        "mongodb+srv://cluster.example.net"
    );
}

/// Resolving a MongoDB connection through the SQL-only `pair` is a typed error.
#[tokio::test]
async fn sql_pair_rejects_mongodb_connection() {
    let resolver = ConnectionResolver::new(parse(MONGO_URI_CONNECTION));
    let error = resolver.pair(Some("mongo")).await.unwrap_err();
    assert!(
        matches!(error, OrmError::Connection(ConnectionError::UnsupportedDriver { ref driver }) if driver.contains("mongodb")),
        "got {error:?}"
    );
}

/// Without the `mongodb` feature, `resolve_any` returns the typed
/// `UnsupportedDriver` naming the feature instead of panicking.
#[cfg(not(feature = "mongodb"))]
#[tokio::test]
async fn resolve_any_without_feature_is_typed_error() {
    let resolver = ConnectionResolver::new(parse(MONGO_URI_CONNECTION));
    let error = resolver.resolve_any(Some("mongo")).await.unwrap_err();
    assert!(
        matches!(error, OrmError::Connection(ConnectionError::UnsupportedDriver { ref driver }) if driver.contains("mongodb") && driver.contains("feature")),
        "got {error:?}"
    );
}

/// With the `mongodb` feature, `resolve_any` returns the Mongo variant and
/// `MongoConfig` is built from the connection entry (env precedence applies).
#[cfg(feature = "mongodb")]
#[tokio::test]
async fn resolve_any_with_feature_returns_mongo() {
    let resolver = ConnectionResolver::new(parse(MONGO_URI_CONNECTION));
    let resolved = resolver
        .resolve_any(Some("mongo"))
        .await
        .expect("resolve mongo");
    assert!(resolved.is_mongo());
    assert!(!resolved.is_sql());
    let client = resolved.as_mongo().expect("mongo client");
    assert_eq!(client.database_name(), "rustasea");
}

/// `MongoConfig::try_from(&ConnectionConfig)` forwards the assembled URI and
/// database, and carries per-connection pool tuning.
#[cfg(feature = "mongodb")]
#[test]
fn mongo_config_from_connection_entry() {
    let connection = ConnectionConfig {
        driver: "mongodb".into(),
        host: Some("db.internal".into()),
        database: Some("app".into()),
        pool: Some(crate::connections::PoolConfig {
            min: Some(1),
            max: Some(5),
            idle_timeout: Some(30),
        }),
        ..ConnectionConfig::default()
    };
    let config = rustasea_mongo::MongoConfig::try_from(&connection).expect("mongo config");
    assert_eq!(config.uri, "mongodb://db.internal:27017/app");
    assert_eq!(config.database, "app");
    let pool = config.pool.expect("pool forwarded");
    assert_eq!(pool.min, Some(1));
    assert_eq!(pool.max, Some(5));
    assert_eq!(pool.idle_timeout, Some(30));
}

/// `resolve_any` still routes SQL connections to the `Sql` variant.
#[tokio::test]
async fn resolve_any_returns_sql_for_sqlite() {
    let resolver = ConnectionResolver::new(parse(MONGO_URI_CONNECTION));
    let resolved = resolver
        .resolve_any(Some("sqlite"))
        .await
        .expect("resolve sql");
    assert!(resolved.is_sql());
    assert!(!resolved.is_mongo());
    assert!(resolved.as_sql().is_some());
}
