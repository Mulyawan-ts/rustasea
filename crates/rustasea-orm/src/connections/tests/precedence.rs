//! Default-selector resolution, pool precedence, and lazy resolver caching.

use super::*;

/// `resolve_url(None)` follows the `default` selector.
#[test]
fn default_selector_switches_connection() {
    let mut config = load_three();
    assert_eq!(
        config.resolve_url(None).unwrap(),
        "sqlite://database.sqlite?mode=rwc"
    );
    config.default = Some("pgsql".to_string());
    assert_eq!(
        config.resolve_url(None).unwrap(),
        "postgres://user:pass@db.example:5433/app"
    );
}

/// The shared `[database.pool]` table feeds runtime pool settings.
#[test]
fn shared_pool_config_becomes_settings() {
    let config = load_three();
    let settings = config.pool_settings("pgsql");
    assert_eq!(settings.min_connections, 1);
    assert_eq!(settings.max_connections, 10);
    assert_eq!(
        settings.idle_timeout,
        Some(std::time::Duration::from_secs(600))
    );
}

/// A per-connection `pool` override wins over the shared table.
#[test]
fn per_connection_pool_overrides_shared() {
    let mut config = load_three();
    config
        .connections
        .get_mut("pgsql")
        .expect("pgsql connection")
        .pool = Some(PoolConfig {
        min: Some(2),
        max: Some(20),
        idle_timeout: None,
    });
    let settings = config.pool_settings("pgsql");
    assert_eq!(settings.min_connections, 2);
    assert_eq!(settings.max_connections, 20);
    assert_eq!(settings.idle_timeout, None);
}

/// The resolver lazily connects and caches one pool per name.
#[tokio::test]
async fn resolver_caches_named_pool() {
    let mut connections = BTreeMap::new();
    connections.insert(
        "mem".to_string(),
        ConnectionConfig {
            driver: "sqlite".into(),
            url: Some("sqlite::memory:".into()),
            ..ConnectionConfig::default()
        },
    );
    let resolver = ConnectionResolver::new(DatabaseConfig {
        default: Some("mem".into()),
        connections,
        ..DatabaseConfig::default()
    });

    assert!(resolver.cached_names().await.is_empty());
    let first = resolver.resolve(None).await.expect("resolve default");
    let second = resolver.resolve(Some("mem")).await.expect("resolve named");
    first.ping().await.expect("ping");
    assert_eq!(first.dialect(), "sqlite");
    assert_eq!(second.dialect(), "sqlite");
    assert_eq!(resolver.cached_names().await, vec!["mem".to_string()]);
}

/// Resolving an unknown name through the resolver is a typed error.
#[tokio::test]
async fn resolver_unknown_name_is_typed_error() {
    let resolver = ConnectionResolver::new(load_three());
    let error = resolver.resolve(Some("ghost")).await.unwrap_err();
    assert!(
        matches!(error, OrmError::Connection(ConnectionError::UnknownConnection(ref n)) if n == "ghost"),
        "got {error:?}"
    );
}
