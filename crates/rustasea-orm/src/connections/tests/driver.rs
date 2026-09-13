//! Driver matrix: named-connection parsing, URL resolution, granular field
//! building, and the typed driver/field errors.

use super::*;

/// The config parser reads three named connections and their fields.
#[test]
fn parses_three_named_connections() {
    let config = load_three();
    assert_eq!(config.default_name(), Some("sqlite"));
    assert_eq!(config.connections.len(), 3);
    assert_eq!(config.connections["pgsql"].driver, "postgres");
    assert_eq!(config.connections["pgsql"].port, Some(5433));
    assert_eq!(
        config.connections["mysql"].host.as_deref(),
        Some("db.example")
    );
}

/// Each named connection resolves to the expected driver-valid URL.
#[test]
fn resolves_each_named_connection_url() {
    let config = load_three();
    assert_eq!(
        config.resolve_url(Some("sqlite")).unwrap(),
        "sqlite://database.sqlite?mode=rwc"
    );
    assert_eq!(
        config.resolve_url(Some("pgsql")).unwrap(),
        "postgres://user:pass@db.example:5433/app"
    );
    assert_eq!(
        config.resolve_url(Some("mysql")).unwrap(),
        "mysql://user@db.example:3306/app"
    );
}

/// Granular fields build correct URLs for every driver.
#[test]
fn granular_fields_build_urls() {
    let sqlite = ConnectionConfig {
        driver: "sqlite".into(),
        database: Some("data/app.sqlite".into()),
        ..ConnectionConfig::default()
    };
    assert_eq!(sqlite.build_url().unwrap(), "sqlite://data/app.sqlite");

    let memory = ConnectionConfig {
        driver: "sqlite".into(),
        database: Some(":memory:".into()),
        ..ConnectionConfig::default()
    };
    assert_eq!(memory.build_url().unwrap(), "sqlite::memory:");

    let postgres = ConnectionConfig {
        driver: "postgres".into(),
        host: Some("127.0.0.1".into()),
        database: Some("rustasea".into()),
        username: Some("rustasea".into()),
        password: Some("secret".into()),
        ..ConnectionConfig::default()
    };
    assert_eq!(
        postgres.build_url().unwrap(),
        "postgres://rustasea:secret@127.0.0.1:5432/rustasea"
    );

    let mysql = ConnectionConfig {
        driver: "mysql".into(),
        host: Some("127.0.0.1".into()),
        database: Some("rustasea".into()),
        username: Some("rustasea".into()),
        charset: Some("utf8mb4".into()),
        ..ConnectionConfig::default()
    };
    assert_eq!(
        mysql.build_url().unwrap(),
        "mysql://rustasea@127.0.0.1:3306/rustasea?charset=utf8mb4"
    );
}

/// A legacy flat `url` (no connections) synthesizes an implicit default.
#[test]
fn legacy_flat_url_synthesizes_default() {
    let dir = TempConfigDir::new();
    dir.write_database("[database]\ndriver = \"postgres\"\nurl = \"postgres://legacy/db\"\n");
    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load config");
    let config = DatabaseConfig::from_loader(&loader).expect("parse");
    assert_eq!(config.default_name(), Some("default"));
    assert_eq!(config.resolve_url(None).unwrap(), "postgres://legacy/db");
    assert_eq!(config.connections["default"].driver, "postgres");
}

/// An unknown connection name is a typed error.
#[test]
fn unknown_connection_is_typed_error() {
    let config = load_three();
    let error = config.resolve_url(Some("nope")).unwrap_err();
    assert!(
        matches!(error, OrmError::Connection(ConnectionError::UnknownConnection(ref n)) if n == "nope"),
        "got {error:?}"
    );
}

/// A missing required granular field is a typed error.
#[test]
fn missing_required_field_is_typed_error() {
    let config = ConnectionConfig {
        driver: "postgres".into(),
        database: Some("rustasea".into()),
        ..ConnectionConfig::default()
    };
    let error = config.build_url().unwrap_err();
    assert!(
        matches!(error, OrmError::Connection(ConnectionError::MissingField { ref field }) if field == "host"),
        "got {error:?}"
    );
}

/// A driver that disagrees with the URL scheme is a typed error.
#[test]
fn driver_url_mismatch_is_typed_error() {
    let config = ConnectionConfig {
        driver: "postgres".into(),
        url: Some("mysql://host:3306/db".into()),
        ..ConnectionConfig::default()
    };
    let error = config.build_url().unwrap_err();
    assert!(
        matches!(error, OrmError::Connection(ConnectionError::DriverMismatch { ref driver, ref scheme }) if driver == "postgres" && scheme == "mysql"),
        "got {error:?}"
    );
}

/// An unrecognised driver is a typed error.
#[test]
fn unsupported_driver_is_typed_error() {
    let config = ConnectionConfig {
        driver: "oracle".into(),
        database: Some("x".into()),
        ..ConnectionConfig::default()
    };
    let error = config.build_url().unwrap_err();
    assert!(
        matches!(error, OrmError::Connection(ConnectionError::UnsupportedDriver { ref driver }) if driver == "oracle"),
        "got {error:?}"
    );
}
