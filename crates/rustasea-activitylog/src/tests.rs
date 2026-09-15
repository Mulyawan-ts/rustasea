//! Integration tests for the activity-log recorder and query API.
//!
//! Each test builds a fresh in-memory SQLite pool with a SQLite-shaped
//! `audit_log` table (the migration DDL targets Postgres; the existing ORM test
//! convention creates BLOB/TEXT columns directly for SQLite), then exercises the
//! recorder + query surface end to end.

use rustasea_orm::activity::{
    clear_activity_recorder, ActivityEvent, ActivityOperation, ActivityRecorder,
};
use rustasea_orm::{DbPool, Migration, Value};
use uuid::Uuid;

use crate::recorder::build_properties;
use crate::{ActivityLogger, CreateAuditLogTable};

/// Create the `audit_log` table with SQLite-appropriate column types.
async fn create_audit_log(pool: &DbPool) {
    pool.execute_bind(
        "CREATE TABLE audit_log (
            id BLOB PRIMARY KEY,
            batch_uuid BLOB,
            log_name TEXT NOT NULL,
            description TEXT NOT NULL,
            subject_type TEXT,
            subject_id BLOB,
            causer_type TEXT,
            causer_id TEXT,
            properties TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        &[],
    )
    .await
    .expect("create audit_log");
}

/// Build a pool with the `audit_log` table applied.
async fn pool_with_audit_log() -> DbPool {
    let pool = DbPool::connect("sqlite::memory:").await.expect("connect");
    create_audit_log(&pool).await;
    pool
}

/// Build a created-event fixture with old/new/changed.
fn created_event(subject_id: Uuid, causer: Option<&str>, batch: Option<Uuid>) -> ActivityEvent {
    ActivityEvent {
        operation: ActivityOperation::Created,
        table: "users".to_string(),
        model_type: "User".to_string(),
        model_id: Some(subject_id.to_string()),
        old: None,
        new: Some(serde_json::json!({ "name": "Ada", "email": "ada@x.test" })),
        changed: vec!["name".to_string(), "email".to_string()],
        causer_id: causer.map(str::to_string),
        batch_uuid: batch.map(|id| id.to_string()),
    }
}

/// Verifies recording an event writes exactly one decodable row.
#[tokio::test]
async fn record_writes_a_row() {
    let pool = pool_with_audit_log().await;
    let logger = ActivityLogger::new(pool.clone());
    let subject_id = Uuid::new_v4();

    let recorded = logger
        .record_event(&created_event(subject_id, Some("user-1"), None))
        .await
        .expect("record");

    assert_eq!(recorded.log_name, "users");
    assert_eq!(recorded.description, "created");
    assert_eq!(recorded.subject_type.as_deref(), Some("User"));
    assert_eq!(recorded.subject_id, Some(subject_id));
    assert_eq!(recorded.causer_id.as_deref(), Some("user-1"));
    pool.close().await;
}

/// Verifies the query API filters by subject, causer, log, and batch.
#[tokio::test]
async fn query_api_filters() {
    let pool = pool_with_audit_log().await;
    let logger = ActivityLogger::new(pool.clone());
    let subject_a = Uuid::new_v4();
    let subject_b = Uuid::new_v4();
    let batch = Uuid::new_v4();

    logger
        .record_event(&created_event(subject_a, Some("user-1"), Some(batch)))
        .await
        .unwrap();
    logger
        .record_event(&created_event(subject_b, Some("user-2"), None))
        .await
        .unwrap();

    let for_a = logger.for_subject("User", subject_a).await.unwrap();
    assert_eq!(for_a.len(), 1);
    assert_eq!(for_a[0].subject_id, Some(subject_a));

    let by_user = logger.caused_by("user-2").await.unwrap();
    assert_eq!(by_user.len(), 1);
    assert_eq!(by_user[0].subject_id, Some(subject_b));

    let in_log = logger.in_log("users").await.unwrap();
    assert_eq!(in_log.len(), 2);

    let in_batch = logger.for_batch(batch).await.unwrap();
    assert_eq!(in_batch.len(), 1);
    assert_eq!(in_batch[0].batch_uuid, Some(batch));

    let latest = logger.latest(1).await.unwrap();
    assert_eq!(latest.len(), 1);

    pool.close().await;
}

/// Verifies the properties payload carries the old/new diff and changed list.
#[tokio::test]
async fn properties_carry_diff_shape() {
    let pool = pool_with_audit_log().await;
    let logger = ActivityLogger::new(pool.clone());
    let subject_id = Uuid::new_v4();

    let event = ActivityEvent {
        operation: ActivityOperation::Updated,
        table: "users".to_string(),
        model_type: "User".to_string(),
        model_id: Some(subject_id.to_string()),
        old: Some(serde_json::json!({ "name": "Ada" })),
        new: Some(serde_json::json!({ "name": "Ada Lovelace" })),
        changed: vec!["name".to_string()],
        causer_id: None,
        batch_uuid: None,
    };
    let recorded = logger.record_event(&event).await.unwrap();
    let properties = recorded.properties.expect("properties present");

    assert_eq!(properties["old"], serde_json::json!({ "name": "Ada" }));
    assert_eq!(
        properties["new"],
        serde_json::json!({ "name": "Ada Lovelace" })
    );
    assert_eq!(properties["changed"], serde_json::json!(["name"]));
    pool.close().await;
}

/// Verifies `old`/`new` sides are omitted when absent (no spurious null).
#[test]
fn build_properties_omits_absent_sides() {
    let event = created_event(Uuid::new_v4(), None, None);
    let properties = build_properties(&event);
    assert!(properties.get("old").is_none());
    assert!(properties.get("new").is_some());
    assert_eq!(properties["changed"], serde_json::json!(["name", "email"]));
}

/// Verifies the recorder can be installed process-wide and cleared.
#[tokio::test]
async fn installs_and_clears_global_recorder() {
    let pool = pool_with_audit_log().await;
    let logger = std::sync::Arc::new(ActivityLogger::new(pool.clone()));

    clear_activity_recorder();
    assert!(rustasea_orm::activity::activity_recorder().is_none());
    crate::install(logger.clone());
    assert!(rustasea_orm::activity::activity_recorder().is_some());
    crate::uninstall();
    assert!(rustasea_orm::activity::activity_recorder().is_none());

    // The logger itself still records directly.
    let subject_id = Uuid::new_v4();
    logger
        .record_event(&created_event(subject_id, None, None))
        .await
        .unwrap();
    assert_eq!(logger.latest(10).await.unwrap().len(), 1);
    pool.close().await;
}

/// Verifies the trait impl (used by the global slot) records a row.
#[tokio::test]
async fn trait_impl_records() {
    let pool = pool_with_audit_log().await;
    let logger = ActivityLogger::new(pool.clone());
    let subject_id = Uuid::new_v4();

    ActivityRecorder::record(&logger, created_event(subject_id, None, None))
        .await
        .expect("trait record");
    assert_eq!(logger.latest(10).await.unwrap().len(), 1);
    pool.close().await;
}

/// Verifies the migration exposes the expected name and DDL shape.
#[test]
fn migration_exposes_ddl() {
    let migration = CreateAuditLogTable;
    assert_eq!(migration.name(), "2027_01_01_000008_create_audit_log_table");
    let up = migration.up().expect("up");
    assert!(up.contains("CREATE TABLE audit_log"));
    assert!(up.contains("properties JSONB"));
    assert!(up.contains("CREATE INDEX audit_log_subject_index"));
    assert!(up.contains("CREATE INDEX audit_log_causer_index"));
    assert!(up.contains("CREATE INDEX audit_log_batch_uuid_index"));
    assert!(up.contains("CREATE INDEX audit_log_log_name_index"));
    let down = migration.down().expect("down");
    assert!(down.contains("DROP TABLE IF EXISTS audit_log"));
}

/// Verifies a null-safe causer: no resolver installed records no actor.
#[tokio::test]
async fn causer_is_null_safe() {
    let pool = pool_with_audit_log().await;
    let logger = ActivityLogger::new(pool.clone());
    rustasea_orm::activity::clear_causer_resolver();

    let subject_id = Uuid::new_v4();
    let recorded = logger
        .record_event(&created_event(subject_id, None, None))
        .await
        .unwrap();
    assert!(recorded.causer_id.is_none());
    pool.close().await;
}

/// Verifies a value bound as a typed UUID round-trips through the recorder.
#[tokio::test]
async fn uuid_columns_round_trip() {
    let pool = pool_with_audit_log().await;
    let logger = ActivityLogger::new(pool.clone());
    let batch = Uuid::new_v4();
    let subject_id = Uuid::new_v4();

    let recorded = logger
        .record_event(&created_event(subject_id, None, Some(batch)))
        .await
        .unwrap();
    assert!(!recorded.id.is_nil());
    assert_eq!(recorded.batch_uuid, Some(batch));

    // Sanity: the raw bind path also stores a UUID.
    let rows = pool
        .fetch_json(
            "SELECT subject_id FROM audit_log WHERE id = $1",
            &[Value::Uuid(recorded.id)],
        )
        .await
        .unwrap();
    assert_eq!(rows[0]["subject_id"], subject_id.to_string());
    pool.close().await;
}
