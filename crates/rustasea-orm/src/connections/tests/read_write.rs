//! Read/write split overlays and the resolver's pool-identity behaviour.

use super::*;

/// A granular read overlay inherits the primary's credentials and database.
#[test]
fn granular_read_overlay_inherits_primary_fields() {
    let config = load_split();
    let connection = &config.connections["pgsql"];
    assert_eq!(
        connection.build_url().unwrap(),
        "postgres://user:pass@primary.internal:5432/app"
    );
    assert_eq!(
        connection.read_url().unwrap(),
        "postgres://user:pass@replica.internal:5432/app"
    );
}

/// A `read.url` overlay replaces the endpoint wholesale.
#[test]
fn url_read_overlay_replaces_endpoint() {
    let config = load_split();
    let connection = &config.connections["mysql"];
    assert_eq!(
        connection.read_url().unwrap(),
        "mysql://user@replica.internal:3306/app"
    );
}

/// Without a split, `read_url` equals the primary write URL.
#[test]
fn unsplit_read_url_falls_back_to_write() {
    let config = load_three();
    let connection = &config.connections["pgsql"];
    assert_eq!(
        connection.read_url().unwrap(),
        connection.build_url().unwrap()
    );
}

/// An invalid read URL is a typed error, not a panic.
#[test]
fn invalid_read_url_is_typed_error() {
    let config = ConnectionConfig {
        driver: "postgres".into(),
        host: Some("primary.internal".into()),
        database: Some("app".into()),
        read: Some(EndpointConfig {
            url: Some("mysql://replica/app".into()),
            ..EndpointConfig::default()
        }),
        ..ConnectionConfig::default()
    };
    let error = config.read_url().unwrap_err();
    assert!(
        matches!(error, OrmError::Connection(ConnectionError::DriverMismatch { ref driver, ref scheme }) if driver == "postgres" && scheme == "mysql"),
        "got {error:?}"
    );
}

/// A read overlay with a blank required field is a typed error.
#[test]
fn read_overlay_missing_field_is_typed_error() {
    let config = ConnectionConfig {
        driver: "postgres".into(),
        host: Some("primary.internal".into()),
        database: Some("app".into()),
        // An explicit-but-blank host fails the required-field check.
        read: Some(EndpointConfig {
            host: Some("   ".into()),
            ..EndpointConfig::default()
        }),
        ..ConnectionConfig::default()
    };
    let error = config.read_url().unwrap_err();
    assert!(
        matches!(error, OrmError::Connection(ConnectionError::MissingField { ref field }) if field == "host"),
        "got {error:?}"
    );
}

/// An unsplit resolver returns the SAME pool for read and write (identity).
#[tokio::test]
async fn unsplit_resolver_reuses_one_pool() {
    let resolver = ConnectionResolver::new(load_three());
    let read = resolver.read(Some("sqlite")).await.expect("read pool");
    let write = resolver.write(Some("sqlite")).await.expect("write pool");
    assert_eq!(
        read.identity(),
        write.identity(),
        "no split must reuse one pool instance"
    );
    let pair = resolver.pair(Some("sqlite")).await.expect("pair");
    assert!(!pair.is_split());
}

/// A split resolver opens two distinct pools for one name.
#[tokio::test]
async fn split_resolver_opens_distinct_pools() {
    let dir = std::env::temp_dir();
    let primary = dir.join(format!(
        "rustasea-orm-rw-primary-{}.sqlite",
        std::process::id()
    ));
    let replica = dir.join(format!(
        "rustasea-orm-rw-replica-{}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&primary);
    let _ = std::fs::remove_file(&replica);

    let mut connections = BTreeMap::new();
    connections.insert(
        "split".to_string(),
        ConnectionConfig {
            driver: "sqlite".into(),
            database: Some(primary.display().to_string()),
            read: Some(EndpointConfig {
                database: Some(replica.display().to_string()),
                ..EndpointConfig::default()
            }),
            ..ConnectionConfig::default()
        },
    );
    let resolver = ConnectionResolver::new(DatabaseConfig {
        default: Some("split".into()),
        connections,
        ..DatabaseConfig::default()
    });

    let pair = resolver.pair(None).await.expect("split pair");
    assert!(pair.is_split(), "two endpoints must yield two pools");
    assert_ne!(pair.read().identity(), pair.write().identity());

    pair.close().await;
    let _ = std::fs::remove_file(&primary);
    let _ = std::fs::remove_file(&replica);
}

/// A read endpoint that cannot be reached surfaces a typed pool error.
#[tokio::test]
async fn unreachable_read_endpoint_is_typed_error() {
    let mut connections = BTreeMap::new();
    connections.insert(
        "broken".to_string(),
        ConnectionConfig {
            driver: "sqlite".into(),
            database: Some(":memory:".into()),
            // A directory path is not a valid SQLite file → connect fails.
            read: Some(EndpointConfig {
                database: Some("/this/path/does/not/exist/db.sqlite".into()),
                ..EndpointConfig::default()
            }),
            ..ConnectionConfig::default()
        },
    );
    let resolver = ConnectionResolver::new(DatabaseConfig {
        default: Some("broken".into()),
        connections,
        ..DatabaseConfig::default()
    });

    let error = resolver.read(None).await.unwrap_err();
    assert!(matches!(error, OrmError::Pool(_)), "got {error:?}");
}
