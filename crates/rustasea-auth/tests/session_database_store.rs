//! Integration tests for the database-backed session store (AUTH-015).
//!
//! Exercises the public [`DatabaseSessionStore`] over a real in-memory SQLite
//! pool (`rustasea_orm::DbPool`) and the `sessions` table shape from the
//! scaffold migration (`create_sessions_table`):
//!
//! * **positive** — login, drop the guard, rebuild a fresh guard over the same
//!   pool, and `parse` still resolves the identity, proving the session survives
//!   a restart (the property the in-memory store cannot offer);
//! * **negative** — a missing `sessions` table surfaces a typed store error
//!   (`AuthError::StoreUnavailable`), never a panic.
//!
//! No environment is mutated, so the crate-wide `ENV_LOCK` is not needed here.
use std::sync::Arc;

use rustasea_auth::users::{AuthUserRecord, MemoryUserRegistry};
use rustasea_auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea_auth::{
    AuthError, Credentials, DatabaseSessionStore, Guard, SessionGuard, SessionPolicy,
};
use rustasea_orm::DbPool;
use tower_sessions::session_store::SessionStore;

/// Plaintext password seeded into the registry.
const PASSWORD: &str = "s3cr3tPass";
/// UUID of the seeded user.
const USER_ID: &str = "user-1";
/// Login email of the seeded user.
const EMAIL: &str = "ada@example.com";

/// `CREATE TABLE` matching the scaffold `create_sessions_table` migration.
const CREATE_SESSIONS: &str = "CREATE TABLE sessions ( \
    id VARCHAR(255) PRIMARY KEY, \
    user_id UUID NULL, \
    payload TEXT NOT NULL, \
    last_activity BIGINT NOT NULL \
);";

/// Open a fresh in-memory SQLite pool with the `sessions` table created.
async fn pool_with_sessions_table() -> DbPool {
    let pool = DbPool::connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite pool connects");
    pool.execute_script(CREATE_SESSIONS)
        .await
        .expect("sessions table created");
    pool
}

/// A credential lookup seeded with one Argon2-hashed user.
fn seeded_lookup() -> Arc<MemoryUserRegistry> {
    let hash = Argon2Verifier::new()
        .hash(PASSWORD)
        .expect("argon2 hashing succeeds");
    let registry = Arc::new(MemoryUserRegistry::default());
    registry.seed(AuthUserRecord {
        id: USER_ID.into(),
        email: EMAIL.into(),
        password_hash: hash,
        email_verified_at: None,
        timezone: None,
    });
    registry
}

/// Build a guard over `pool` with the seeded credential lookup.
fn guard_over(pool: &DbPool) -> SessionGuard<DatabaseSessionStore> {
    let store = Arc::new(DatabaseSessionStore::new(pool.clone(), "sessions"));
    SessionGuard::with_store(SessionPolicy::default(), store).with_lookup(seeded_lookup())
}

/// Credentials for the seeded user.
fn credentials() -> Credentials {
    Credentials {
        email: EMAIL.into(),
        password: PASSWORD.into(),
    }
}

/// A login persisted by one guard is resolved by a **rebuilt** guard over the
/// same pool — the session survives the guard instance being dropped.
#[tokio::test]
async fn session_survives_guard_rebuild() {
    let pool = pool_with_sessions_table().await;

    let token = {
        let guard = guard_over(&pool);
        guard
            .login(&credentials())
            .await
            .expect("valid credentials log in")
            .access_token
    };

    // Rebuild a *fresh* guard instance over the same pool (a new process would
    // open a new pool against the same database).
    let rebuilt = guard_over(&pool);
    let principal = rebuilt
        .parse(&token)
        .await
        .expect("session persisted by the dropped guard still parses");
    assert_eq!(principal.id, USER_ID);
    assert_eq!(principal.email.as_deref(), Some(EMAIL));
    assert_eq!(principal.guard, "session");
}

/// Logout through a rebuilt guard destroys the persisted row, so a replayed
/// token is rejected — the store really owns the session, not the guard.
#[tokio::test]
async fn logout_through_rebuilt_guard_invalidates_the_row() {
    let pool = pool_with_sessions_table().await;
    let token = {
        let guard = guard_over(&pool);
        guard
            .login(&credentials())
            .await
            .expect("login succeeds")
            .access_token
    };

    let rebuilt = guard_over(&pool);
    rebuilt.logout(&token).await.expect("logout succeeds");
    let err = rebuilt
        .parse(&token)
        .await
        .expect_err("replayed token is rejected");
    assert_eq!(err, AuthError::InvalidToken);
}

/// A missing `sessions` table surfaces a typed store error, not a panic.
#[tokio::test]
async fn missing_sessions_table_is_a_typed_store_error() {
    // A pool *without* the table created.
    let pool = DbPool::connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite pool connects");

    // Direct store access surfaces the typed backend error.
    let store = DatabaseSessionStore::new(pool.clone(), "sessions");
    let id = tower_sessions::session::Id::default();
    let err = store
        .load(&id)
        .await
        .expect_err("loading from a missing table is an error");
    assert!(
        matches!(err, tower_sessions::session_store::Error::Backend(_)),
        "got {err:?}"
    );

    // The guard maps it onto the typed `StoreUnavailable` variant.
    let guard = guard_over(&pool);
    let err = guard
        .login(&credentials())
        .await
        .expect_err("login against a missing table fails closed");
    assert_eq!(err, AuthError::StoreUnavailable);
}

/// An invalid table identifier is rejected before any SQL runs (no injection).
#[tokio::test]
async fn invalid_table_name_is_a_typed_store_error() {
    let pool = pool_with_sessions_table().await;
    let store = DatabaseSessionStore::new(pool, "sessions; DROP TABLE users");
    let id = tower_sessions::session::Id::default();
    let err = store
        .load(&id)
        .await
        .expect_err("invalid table name is rejected");
    assert!(
        matches!(err, tower_sessions::session_store::Error::Backend(_)),
        "got {err:?}"
    );
}

/// The config-driven constructor resolves the connection and builds a store
/// that round-trips a record through the ORM pool.
#[tokio::test]
async fn from_config_resolves_connection_and_round_trips() {
    use rustasea_auth::SessionConfig;
    use rustasea_orm::{ConnectionConfig, ConnectionResolver, DatabaseConfig};
    use std::collections::BTreeMap;

    let mut connections = BTreeMap::new();
    connections.insert(
        "default".to_string(),
        ConnectionConfig {
            driver: "sqlite".to_string(),
            url: Some("sqlite::memory:".to_string()),
            ..ConnectionConfig::default()
        },
    );
    let resolver = ConnectionResolver::new(DatabaseConfig {
        default: Some("default".to_string()),
        connections,
        ..DatabaseConfig::default()
    });

    let config = SessionConfig {
        driver: "database".to_string(),
        connection: "default".to_string(),
        table: "sessions".to_string(),
        ..SessionConfig::default()
    };
    // The driver validates and the store builds over the resolved connection.
    assert!(config.is_database_driver());
    config
        .validate_driver()
        .expect("database driver is accepted");

    let store = DatabaseSessionStore::from_config(&config, &resolver)
        .await
        .expect("store builds from config");
    assert_eq!(store.table(), "sessions");

    // The resolved in-memory pool needs the table; create it, then round-trip.
    store
        .pool()
        .execute_script(CREATE_SESSIONS)
        .await
        .expect("sessions table created");

    let guard = SessionGuard::with_store(SessionPolicy::default(), Arc::new(store))
        .with_lookup(seeded_lookup());
    let token = guard
        .login(&credentials())
        .await
        .expect("login succeeds over the config-built store")
        .access_token;
    let principal = guard.parse(&token).await.expect("token parses");
    assert_eq!(principal.id, USER_ID);
}
