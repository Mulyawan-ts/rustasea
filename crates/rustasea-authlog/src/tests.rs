//! Integration tests for the authentication-log recorder and query API.
//!
//! Each test builds a fresh in-memory SQLite pool with a SQLite-shaped
//! `authentication_log` table (the migration DDL targets Postgres; the existing
//! ORM test convention creates TEXT/INTEGER columns directly for SQLite), then
//! exercises the recorder + query surface end to end.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rustasea_orm::{DbPool, Migration};

use crate::event::{AuthLogEvent, AuthLogEventKind};
use crate::notifier::NewDeviceNotifier;
use crate::{AuthenticationLogLogger, CreateAuthenticationLogTable, Result};

/// Create the `authentication_log` table with SQLite-appropriate column types.
async fn create_authentication_log(pool: &DbPool) {
    pool.execute_script(
        "CREATE TABLE authentication_log (
            id BLOB PRIMARY KEY,
            user_id TEXT,
            email TEXT,
            guard_name TEXT,
            event TEXT NOT NULL,
            ip_address TEXT,
            user_agent TEXT,
            successful INTEGER NOT NULL,
            login_at TEXT,
            logout_at TEXT,
            cleared_by_user_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
    )
    .await
    .expect("create authentication_log");
}

/// Build a pool with the `authentication_log` table applied.
async fn pool_with_table() -> DbPool {
    let pool = DbPool::connect("sqlite::memory:").await.expect("connect");
    create_authentication_log(&pool).await;
    pool
}

/// Build a successful-login event for `user_id` from `ip`.
fn success(user_id: &str, ip: &str) -> AuthLogEvent {
    AuthLogEvent::login_succeeded(
        user_id,
        Some("ada@example.com".to_string()),
        Some("session".to_string()),
        Some(ip.to_string()),
        Some("test-agent".to_string()),
    )
}

/// Verifies a successful login writes a row with the expected fields.
#[tokio::test]
async fn records_successful_login() {
    let pool = pool_with_table().await;
    let logger = AuthenticationLogLogger::new(pool.clone());

    logger.record(&success("user-1", "1.2.3.4")).await.unwrap();

    let rows = logger.latest(10).await.unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.user_id.as_deref(), Some("user-1"));
    assert_eq!(row.email.as_deref(), Some("ada@example.com"));
    assert_eq!(row.guard_name.as_deref(), Some("session"));
    assert_eq!(row.event, "login_succeeded");
    assert_eq!(row.ip_address.as_deref(), Some("1.2.3.4"));
    assert_eq!(row.user_agent.as_deref(), Some("test-agent"));
    assert!(row.successful);
    assert!(row.login_at.is_some());
    assert!(row.logout_at.is_none());
    pool.close().await;
}

/// Verifies a failed login writes `successful = false` with no login time.
#[tokio::test]
async fn records_failed_login() {
    let pool = pool_with_table().await;
    let logger = AuthenticationLogLogger::new(pool.clone());

    let event = AuthLogEvent::login_failed(
        Some("nobody@example.com".to_string()),
        Some("session".to_string()),
        Some("1.2.3.4".to_string()),
        None,
    );
    logger.record(&event).await.unwrap();

    let rows = logger
        .for_event(AuthLogEventKind::LoginFailed)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].successful);
    assert_eq!(rows[0].event, "login_failed");
    assert!(rows[0].user_id.is_none());
    assert!(rows[0].login_at.is_some());
    pool.close().await;
}

/// Verifies a lockout writes `successful = false` with the `lockout` event.
#[tokio::test]
async fn records_lockout() {
    let pool = pool_with_table().await;
    let logger = AuthenticationLogLogger::new(pool.clone());

    let event = AuthLogEvent::lockout(
        Some("ada@example.com".to_string()),
        Some("session".to_string()),
        Some("9.9.9.9".to_string()),
        None,
    );
    logger.record(&event).await.unwrap();

    let rows = logger.for_event(AuthLogEventKind::Lockout).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].successful);
    assert_eq!(rows[0].event, "lockout");
    assert_eq!(rows[0].ip_address.as_deref(), Some("9.9.9.9"));
    pool.close().await;
}

/// Verifies a logout fills `logout_at` on the latest open login row.
#[tokio::test]
async fn logout_fills_open_row() {
    let pool = pool_with_table().await;
    let logger = AuthenticationLogLogger::new(pool.clone());

    logger.record(&success("user-1", "1.2.3.4")).await.unwrap();
    let logout = AuthLogEvent::logout(
        Some("user-1".to_string()),
        Some("session".to_string()),
        Some("1.2.3.4".to_string()),
        Some("test-agent".to_string()),
    );
    logger.record(&logout).await.unwrap();

    let rows = logger.for_user("user-1").await.unwrap();
    assert_eq!(rows.len(), 1, "logout must update, not insert a second row");
    assert!(rows[0].logout_at.is_some());
    assert!(rows[0].login_at.is_some());
    pool.close().await;
}

/// Verifies a logout with no open row inserts a standalone logout row.
#[tokio::test]
async fn logout_without_open_row_inserts_standalone() {
    let pool = pool_with_table().await;
    let logger = AuthenticationLogLogger::new(pool.clone());

    let logout = AuthLogEvent::logout(
        Some("ghost".to_string()),
        Some("session".to_string()),
        Some("1.2.3.4".to_string()),
        None,
    );
    logger.record(&logout).await.unwrap();

    let rows = logger.for_user("ghost").await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].event, "logout");
    assert!(rows[0].logout_at.is_some());
    assert!(rows[0].login_at.is_none());
    assert!(rows[0].successful);
    pool.close().await;
}

/// Verifies the query filters (`for_user`, `for_event`, `latest`).
#[tokio::test]
async fn query_api_filters() {
    let pool = pool_with_table().await;
    let logger = AuthenticationLogLogger::new(pool.clone());

    logger.record(&success("user-1", "1.2.3.4")).await.unwrap();
    logger.record(&success("user-2", "5.6.7.8")).await.unwrap();
    logger
        .record(&AuthLogEvent::login_failed(
            Some("ada@example.com".to_string()),
            None,
            Some("1.2.3.4".to_string()),
            None,
        ))
        .await
        .unwrap();

    let for_one = logger.for_user("user-1").await.unwrap();
    assert_eq!(for_one.len(), 1);
    assert_eq!(for_one[0].user_id.as_deref(), Some("user-1"));

    let successes = logger
        .for_event(AuthLogEventKind::LoginSucceeded)
        .await
        .unwrap();
    assert_eq!(successes.len(), 2);

    let latest = logger.latest(2).await.unwrap();
    assert_eq!(latest.len(), 2);
    pool.close().await;
}

/// Verifies a null-safe record: no user id, email, IP, or user agent.
#[tokio::test]
async fn null_fields_are_safe() {
    let pool = pool_with_table().await;
    let logger = AuthenticationLogLogger::new(pool.clone());

    let event = AuthLogEvent::login_failed(
        None::<String>,
        None::<String>,
        None::<String>,
        None::<String>,
    );
    logger.record(&event).await.unwrap();

    let rows = logger.latest(10).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].user_id.is_none());
    assert!(rows[0].email.is_none());
    assert!(rows[0].ip_address.is_none());
    assert!(rows[0].user_agent.is_none());
    pool.close().await;
}

/// A recording notifier fixture capturing every invocation.
#[derive(Default)]
struct RecordingNotifier {
    calls: Mutex<Vec<(String, Option<String>, String)>>,
}

#[async_trait]
impl NewDeviceNotifier for RecordingNotifier {
    async fn notify(
        &self,
        user_id: &str,
        email: Option<&str>,
        ip_address: &str,
        _user_agent: Option<&str>,
    ) -> Result<()> {
        self.calls.lock().unwrap().push((
            user_id.to_string(),
            email.map(str::to_string),
            ip_address.to_string(),
        ));
        Ok(())
    }
}

/// Verifies the new-device notifier fires once per unseen IP, never twice.
#[tokio::test]
async fn notifier_fires_only_for_new_ip() {
    let pool = pool_with_table().await;
    let notifier = Arc::new(RecordingNotifier::default());
    let logger = AuthenticationLogLogger::new(pool.clone()).with_notifier(notifier.clone());

    logger.record(&success("user-1", "1.2.3.4")).await.unwrap();
    assert_eq!(
        notifier.calls.lock().unwrap().len(),
        1,
        "first login notifies"
    );

    logger.record(&success("user-1", "1.2.3.4")).await.unwrap();
    assert_eq!(
        notifier.calls.lock().unwrap().len(),
        1,
        "a repeat login from the same IP must not notify again"
    );

    logger.record(&success("user-1", "5.6.7.8")).await.unwrap();
    assert_eq!(
        notifier.calls.lock().unwrap().len(),
        2,
        "a login from a new IP notifies again"
    );
    pool.close().await;
}

/// Verifies the global slot installs, records, and clears.
#[tokio::test]
async fn global_slot_installs_and_clears() {
    let pool = pool_with_table().await;
    let logger = Arc::new(AuthenticationLogLogger::new(pool.clone()));

    crate::clear();
    assert!(crate::logger().is_none());
    // With no logger installed, `record_event` is a no-op.
    crate::record_event(&success("user-1", "1.2.3.4"))
        .await
        .unwrap();

    crate::install(logger.clone());
    assert!(crate::logger().is_some());
    crate::record_event(&success("user-1", "1.2.3.4"))
        .await
        .unwrap();
    assert_eq!(logger.latest(10).await.unwrap().len(), 1);

    crate::clear();
    assert!(crate::logger().is_none());
    pool.close().await;
}

/// Verifies the event-kind strings and round-trip parsing are stable.
#[test]
fn event_kind_strings_are_stable() {
    for (kind, text) in [
        (AuthLogEventKind::LoginSucceeded, "login_succeeded"),
        (AuthLogEventKind::LoginFailed, "login_failed"),
        (AuthLogEventKind::Lockout, "lockout"),
        (AuthLogEventKind::Logout, "logout"),
    ] {
        assert_eq!(kind.as_str(), text);
        assert_eq!(AuthLogEventKind::parse(text), Some(kind));
    }
    assert_eq!(AuthLogEventKind::parse("unknown"), None);
}

/// Verifies the migration exposes the expected name and DDL shape.
#[test]
fn migration_exposes_ddl() {
    let migration = CreateAuthenticationLogTable;
    assert_eq!(
        migration.name(),
        "2027_01_01_000009_create_authentication_log_table"
    );
    let up = migration.up().expect("up");
    assert!(up.contains("CREATE TABLE authentication_log"));
    assert!(up.contains("user_id VARCHAR(255) NULL"));
    assert!(up.contains("successful BOOLEAN NOT NULL"));
    assert!(up.contains("CREATE INDEX authentication_log_user_id_index"));
    assert!(up.contains("CREATE INDEX authentication_log_event_index"));
    assert!(up.contains("CREATE INDEX authentication_log_ip_address_index"));
    let down = migration.down().expect("down");
    assert!(down.contains("DROP TABLE IF EXISTS authentication_log"));
}
