//! Activity-log hook integration tests (M2).
//!
//! Exercises the ORM write-path hooks against a real in-memory SQLite pool with
//! a test recorder installed process-wide. Covers: create/update/delete emit
//! events only for opted-in models, a no-op update is skipped under
//! `only_dirty`, the causer resolver is applied, redacted columns never reach
//! the event, and a non-opted model emits nothing.

use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Utc};
use rustasea_orm::activity::{
    clear_activity_recorder, clear_causer_resolver, register_activity_recorder, ActivityEvent,
    ActivityLogError, ActivityOperation, ActivityRecorder,
};
use rustasea_orm::{DbPool, Model, ModelOps};
use uuid::Uuid;

/// Serialises tests that install the process-wide recorder/causer slots.
///
/// The registry is global, so parallel tests would observe each other's
/// recorder. Each async test holds this guard for its duration.
fn test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// A recorder that captures events in memory for assertions.
#[derive(Default)]
struct CapturingRecorder {
    events: Mutex<Vec<ActivityEvent>>,
}

impl CapturingRecorder {
    /// The captured events, cloned out of the lock.
    fn events(&self) -> Vec<ActivityEvent> {
        self.events.lock().expect("events lock").clone()
    }
}

#[async_trait::async_trait]
impl ActivityRecorder for CapturingRecorder {
    async fn record(&self, event: ActivityEvent) -> Result<(), ActivityLogError> {
        self.events.lock().expect("events lock").push(event);
        Ok(())
    }
}

/// An opted-in model: logs everything except `password`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, rustasea_macros::Model)]
#[logs_activity(except = "password")]
struct Account {
    id: Uuid,
    name: String,
    password: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

/// An opted-in model that skips no-op updates.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, rustasea_macros::Model)]
#[logs_activity(only_dirty = "true")]
struct Note {
    id: Uuid,
    body: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

/// A field-level redaction: `secret` is skipped, everything else logged.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, rustasea_macros::Model)]
#[logs_activity]
struct Vault {
    id: Uuid,
    label: String,
    #[logs_activity(skip)]
    secret: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

/// A model that never opted into activity logging.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, rustasea_macros::Model)]
struct Silent {
    id: Uuid,
    value: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

/// Create `table` with the standard model columns.
async fn create_table(pool: &DbPool, table: &str) {
    let sql = format!(
        "CREATE TABLE {table} (
            id BLOB PRIMARY KEY,
            name TEXT,
            password TEXT,
            body TEXT,
            label TEXT,
            secret TEXT,
            value TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            deleted_at TEXT
        )"
    );
    pool.execute_bind(&sql, &[]).await.expect("create table");
}

/// Build a fresh in-memory pool with the shared table applied.
async fn pool() -> DbPool {
    let pool = DbPool::connect("sqlite::memory:").await.expect("connect");
    create_table(&pool, "accounts").await;
    create_table(&pool, "notes").await;
    create_table(&pool, "vaults").await;
    create_table(&pool, "silents").await;
    pool
}

/// An unsaved `Account`.
fn account(name: &str, password: &str) -> Account {
    let now = Utc::now();
    Account {
        id: Uuid::now_v7(),
        name: name.to_string(),
        password: password.to_string(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
    }
}

/// Verifies create/update/delete emit events with subject, diff, and causer.
#[tokio::test]
async fn opted_in_model_emits_events() {
    let _guard = test_lock().lock().await;
    let pool = pool().await;
    let recorder = Arc::new(CapturingRecorder::default());
    register_activity_recorder(recorder.clone());
    rustasea_orm::activity::set_causer_resolver(Arc::new(|| Some("actor-1".to_string())));

    let created = Account::create(&pool, account("Ada", "hunter2"))
        .await
        .expect("create");
    let mut updated = created.clone();
    updated.name = "Ada Lovelace".to_string();
    Account::update(&pool, updated).await.expect("update");
    Account::delete(&pool, created.id).await.expect("delete");

    let events = recorder.events();
    assert_eq!(events.len(), 3, "create + update + delete");
    assert_eq!(events[0].operation, ActivityOperation::Created);
    assert_eq!(events[0].model_type, "Account");
    assert_eq!(
        events[0].model_id.as_deref(),
        Some(created.id.to_string().as_str())
    );
    assert_eq!(events[0].causer_id.as_deref(), Some("actor-1"));

    assert_eq!(events[1].operation, ActivityOperation::Updated);
    assert!(
        events[1].changed.contains(&"name".to_string()),
        "changed should include name: {:?}",
        events[1].changed
    );
    assert_eq!(events[2].operation, ActivityOperation::Deleted);

    clear_causer_resolver();
    clear_activity_recorder();
    pool.close().await;
}

/// Verifies a redacted column never appears in the event payload.
#[tokio::test]
async fn redacted_columns_never_reach_the_event() {
    let _guard = test_lock().lock().await;
    let pool = pool().await;
    let recorder = Arc::new(CapturingRecorder::default());
    register_activity_recorder(recorder.clone());

    Account::create(&pool, account("Grace", "top-secret"))
        .await
        .expect("create");

    let events = recorder.events();
    let new = events[0].new.as_ref().expect("new snapshot");
    assert!(new.get("password").is_none(), "password must be redacted");
    assert_eq!(new.get("name").and_then(|v| v.as_str()), Some("Grace"));

    clear_activity_recorder();
    pool.close().await;
}

/// Verifies field-level `#[logs_activity(skip)]` redaction.
#[tokio::test]
async fn field_level_skip_redacts() {
    let _guard = test_lock().lock().await;
    let pool = pool().await;
    let recorder = Arc::new(CapturingRecorder::default());
    register_activity_recorder(recorder.clone());

    let now = Utc::now();
    Vault::create(
        &pool,
        Vault {
            id: Uuid::now_v7(),
            label: "prod".to_string(),
            secret: "shh".to_string(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
        },
    )
    .await
    .expect("create");

    let events = recorder.events();
    let new = events[0].new.as_ref().expect("new snapshot");
    assert!(new.get("secret").is_none(), "secret must be skipped");
    assert_eq!(new.get("label").and_then(|v| v.as_str()), Some("prod"));

    clear_activity_recorder();
    pool.close().await;
}

/// Verifies a no-op update writes no event when `only_dirty` is set.
#[tokio::test]
async fn only_dirty_skips_noop_update() {
    let _guard = test_lock().lock().await;
    let pool = pool().await;
    let recorder = Arc::new(CapturingRecorder::default());
    register_activity_recorder(recorder.clone());

    let now = Utc::now();
    let created = Note::create(
        &pool,
        Note {
            id: Uuid::now_v7(),
            body: "hello".to_string(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
        },
    )
    .await
    .expect("create");
    // Re-save with identical attributes: no user column changes.
    Note::update(&pool, created.clone()).await.expect("update");

    let events = recorder.events();
    assert_eq!(events.len(), 1, "only the create should be recorded");

    clear_activity_recorder();
    pool.close().await;
}

/// Verifies a non-opted model emits nothing even with a recorder installed.
#[tokio::test]
async fn non_opted_model_emits_nothing() {
    let _guard = test_lock().lock().await;
    let pool = pool().await;
    let recorder = Arc::new(CapturingRecorder::default());
    register_activity_recorder(recorder.clone());

    let now = Utc::now();
    let created = Silent::create(
        &pool,
        Silent {
            id: Uuid::now_v7(),
            value: "x".to_string(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
        },
    )
    .await
    .expect("create");
    Silent::delete(&pool, created.id).await.expect("delete");

    assert!(recorder.events().is_empty(), "Silent never opted in");

    clear_activity_recorder();
    pool.close().await;
}

/// Verifies the derive emits the expected opt-in + column policy.
#[test]
fn derive_emits_activity_policy() {
    assert!(<Account as Model>::logs_activity());
    assert!(!<Silent as Model>::logs_activity());
    assert!(<Note as Model>::activity_log_only_dirty());

    use rustasea_orm::activity::ActivityColumns;
    assert_eq!(
        <Account as Model>::activity_columns(),
        ActivityColumns::except(vec!["password".to_string()])
    );
    assert_eq!(
        <Vault as Model>::activity_columns(),
        ActivityColumns::except(vec!["secret".to_string()])
    );
}
