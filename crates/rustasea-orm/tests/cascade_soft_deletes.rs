//! Cascade soft-delete integration tests (ADOPT-018).
//!
//! Exercises the declarative cascade engine against a real in-memory SQLite
//! pool: a parent soft delete / restore / force delete fans out to the rows of
//! its declared relations only, in chunks, with a cycle guard and a restore
//! provenance guard. Also covers the typed error for an undeclared cascade
//! relation, the single-level limitation, and the shared activity `batch_uuid`.
//!
//! Cascade is opt-in via `#[cascade_soft_deletes(...)]`, but `relations()` is
//! hand-written (the derive does not emit it), so the runtime models here
//! override `cascade_soft_deletes`/`cascade_relations` by hand — exactly what
//! the derive emits. The derive attribute itself is exercised by `Ghost`.

use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, SecondsFormat, Utc};
use rustasea_orm::activity::{
    clear_activity_recorder, register_activity_recorder, ActivityEvent, ActivityLogError,
    ActivityOperation, ActivityRecorder,
};
use rustasea_orm::{DbPool, Model, ModelOps, OrmError, Relation, Value};
use uuid::Uuid;

// Sibling-relation and self-reference tests live in a submodule so this file
// stays within the 500-line cap. An explicit `path` is required because this
// integration-test file is a crate root, not a `mod.rs`.
#[path = "cascade_soft_deletes/relations.rs"]
mod relations;

/// Serialises the tests that share the process-wide batch/recorder slots.
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

/// Current time as a fixed-width RFC3339 string (the SQLite datetime shape).
fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// A user that cascades its soft delete to `posts` and logs activity.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct User {
    id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

impl Model for User {
    fn type_name() -> &'static str {
        "User"
    }
    fn primary_key(&self) -> Uuid {
        self.id
    }
    fn assign_id(&mut self) -> Uuid {
        self.id = Uuid::now_v7();
        self.id
    }
    fn relations() -> Vec<Relation> {
        vec![
            Relation::has_many("posts", "posts", "User"),
            // A second relation to the SAME `posts` table with a different FK:
            // both must cascade, proving the visited set keys by relation name.
            Relation::has_many_composite("archived_posts", "posts", &["archived_by"], &["id"])
                .expect("valid archived_posts relation"),
            // Declared for eager loading but intentionally NOT cascaded.
            Relation::has_many("invitations", "invitations", "User"),
        ]
    }
    fn cascade_soft_deletes() -> bool {
        true
    }
    fn cascade_relations() -> &'static [&'static str] {
        &["posts", "archived_posts"]
    }
    fn logs_activity() -> bool {
        true
    }
}

/// A self-referential category (guarded by the visited set).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Category {
    id: Uuid,
    category_id: Option<Uuid>,
    name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

impl Model for Category {
    fn type_name() -> &'static str {
        "Category"
    }
    fn primary_key(&self) -> Uuid {
        self.id
    }
    fn assign_id(&mut self) -> Uuid {
        self.id = Uuid::now_v7();
        self.id
    }
    fn relations() -> Vec<Relation> {
        vec![Relation::has_many("children", "categories", "Category")]
    }
    fn cascade_soft_deletes() -> bool {
        true
    }
    fn cascade_relations() -> &'static [&'static str] {
        &["children"]
    }
}

/// A derived model opting into cascade without declaring `relations()`, used to
/// prove the derive overrides and the undeclared-relation error.
#[derive(rustasea_macros::Model)]
#[cascade_soft_deletes("posts")]
#[allow(dead_code)]
struct Ghost {
    id: Uuid,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

/// A derived model that does not opt into cascading.
#[derive(rustasea_macros::Model)]
#[allow(dead_code)]
struct Plain {
    id: Uuid,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

/// Build a fresh in-memory pool with every cascade table.
async fn pool() -> DbPool {
    let pool = DbPool::connect("sqlite::memory:").await.expect("connect");
    pool.execute_script(
        "CREATE TABLE users (id BLOB PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT);
         CREATE TABLE posts (id BLOB PRIMARY KEY, user_id BLOB NOT NULL, archived_by BLOB, title TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT);
         CREATE TABLE comments (id BLOB PRIMARY KEY, post_id BLOB NOT NULL, body TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT);
         CREATE TABLE invitations (id BLOB PRIMARY KEY, user_id BLOB NOT NULL, note TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT);
         CREATE TABLE categories (id BLOB PRIMARY KEY, category_id BLOB, name TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT);
         CREATE TABLE ghosts (id BLOB PRIMARY KEY, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT);",
    )
    .await
    .expect("create tables");
    pool
}

/// Insert a user row.
async fn insert_user(pool: &DbPool, id: Uuid) {
    let s = now();
    let sql = "INSERT INTO users (id, name, created_at, updated_at) VALUES ($1, $2, $3, $4)";
    let values = [
        Value::Uuid(id),
        Value::Text("Ada".into()),
        Value::Text(s.clone()),
        Value::Text(s),
    ];
    pool.execute_bind(sql, &values).await.expect("insert user");
}

/// Insert a post row owned by `user_id`.
async fn insert_post(pool: &DbPool, id: Uuid, user_id: Uuid) {
    let s = now();
    let sql = "INSERT INTO posts (id, user_id, title, created_at, updated_at) VALUES ($1, $2, $3, $4, $5)";
    let values = [
        Value::Uuid(id),
        Value::Uuid(user_id),
        Value::Text("hello".into()),
        Value::Text(s.clone()),
        Value::Text(s),
    ];
    pool.execute_bind(sql, &values).await.expect("insert post");
}

/// Insert a comment row owned by `post_id`.
async fn insert_comment(pool: &DbPool, id: Uuid, post_id: Uuid) {
    let s = now();
    let sql = "INSERT INTO comments (id, post_id, body, created_at, updated_at) VALUES ($1, $2, $3, $4, $5)";
    let values = [
        Value::Uuid(id),
        Value::Uuid(post_id),
        Value::Text("nice".into()),
        Value::Text(s.clone()),
        Value::Text(s),
    ];
    pool.execute_bind(sql, &values)
        .await
        .expect("insert comment");
}

/// Insert an invitation row owned by `user_id`.
async fn insert_invitation(pool: &DbPool, id: Uuid, user_id: Uuid) {
    let s = now();
    let sql = "INSERT INTO invitations (id, user_id, note, created_at, updated_at) VALUES ($1, $2, $3, $4, $5)";
    let values = [
        Value::Uuid(id),
        Value::Uuid(user_id),
        Value::Text("join".into()),
        Value::Text(s.clone()),
        Value::Text(s),
    ];
    pool.execute_bind(sql, &values)
        .await
        .expect("insert invitation");
}

/// Insert a category row with an optional parent.
async fn insert_category(pool: &DbPool, id: Uuid, parent: Option<Uuid>) {
    let s = now();
    let sql = "INSERT INTO categories (id, category_id, name, created_at, updated_at) VALUES ($1, $2, $3, $4, $5)";
    let values = [
        Value::Uuid(id),
        parent.map_or(Value::Null, Value::Uuid),
        Value::Text("cat".into()),
        Value::Text(s.clone()),
        Value::Text(s),
    ];
    pool.execute_bind(sql, &values)
        .await
        .expect("insert category");
}

/// The `deleted_at` value of a row, or `None` when the row is active/missing.
async fn deleted_at(pool: &DbPool, table: &str, id: Uuid) -> Option<String> {
    let sql = format!("SELECT deleted_at FROM {table} WHERE id = $1");
    let rows = pool
        .fetch_json(&sql, &[Value::Uuid(id)])
        .await
        .expect("fetch deleted_at");
    rows.first()
        .and_then(|row| row.get("deleted_at"))
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

/// Whether a row exists (regardless of soft-delete state).
async fn row_exists(pool: &DbPool, table: &str, id: Uuid) -> bool {
    let sql = format!("SELECT id FROM {table} WHERE id = $1");
    !pool
        .fetch_json(&sql, &[Value::Uuid(id)])
        .await
        .expect("fetch id")
        .is_empty()
}

/// Verifies a parent soft delete cascades to its declared children only.
#[tokio::test]
async fn soft_delete_cascades_to_declared_children() {
    let _guard = test_lock().lock().await;
    let pool = pool().await;
    let user = Uuid::now_v7();
    let (post_a, post_b) = (Uuid::now_v7(), Uuid::now_v7());
    let comment = Uuid::now_v7();
    let invitation = Uuid::now_v7();
    insert_user(&pool, user).await;
    insert_post(&pool, post_a, user).await;
    insert_post(&pool, post_b, user).await;
    insert_comment(&pool, comment, post_a).await;
    insert_invitation(&pool, invitation, user).await;

    assert!(User::soft_delete(&pool, user).await.expect("soft delete"));

    assert!(deleted_at(&pool, "users", user).await.is_some());
    assert!(deleted_at(&pool, "posts", post_a).await.is_some());
    assert!(deleted_at(&pool, "posts", post_b).await.is_some());
    // Single level: the grandchild comment and the undeclared invitation stay.
    assert!(deleted_at(&pool, "comments", comment).await.is_none());
    assert!(deleted_at(&pool, "invitations", invitation).await.is_none());
    pool.close().await;
}

/// Verifies relations not listed in `cascade_relations` are left untouched.
#[tokio::test]
async fn soft_delete_leaves_undeclared_relations_untouched() {
    let _guard = test_lock().lock().await;
    let pool = pool().await;
    let user = Uuid::now_v7();
    let post = Uuid::now_v7();
    let invitation = Uuid::now_v7();
    insert_user(&pool, user).await;
    insert_post(&pool, post, user).await;
    insert_invitation(&pool, invitation, user).await;

    User::soft_delete(&pool, user).await.expect("soft delete");

    assert!(deleted_at(&pool, "posts", post).await.is_some());
    assert!(
        deleted_at(&pool, "invitations", invitation).await.is_none(),
        "invitations is declared but not cascaded"
    );
    pool.close().await;
}

/// Verifies restoring a parent restores the children deleted with it.
#[tokio::test]
async fn restore_cascades_to_children() {
    let _guard = test_lock().lock().await;
    let pool = pool().await;
    let user = Uuid::now_v7();
    let (post_a, post_b) = (Uuid::now_v7(), Uuid::now_v7());
    insert_user(&pool, user).await;
    insert_post(&pool, post_a, user).await;
    insert_post(&pool, post_b, user).await;

    User::soft_delete(&pool, user).await.expect("soft delete");
    assert!(User::restore(&pool, user).await.expect("restore"));

    assert!(deleted_at(&pool, "users", user).await.is_none());
    assert!(deleted_at(&pool, "posts", post_a).await.is_none());
    assert!(deleted_at(&pool, "posts", post_b).await.is_none());
    pool.close().await;
}

/// Verifies a restore does not resurrect a row deleted independently earlier.
#[tokio::test]
async fn restore_does_not_resurrect_independently_deleted() {
    let _guard = test_lock().lock().await;
    let pool = pool().await;
    let user = Uuid::now_v7();
    let (independent, cascaded) = (Uuid::now_v7(), Uuid::now_v7());
    insert_user(&pool, user).await;
    insert_post(&pool, independent, user).await;
    insert_post(&pool, cascaded, user).await;
    // Delete one post on its own, with an earlier stamp.
    pool.execute_bind(
        "UPDATE posts SET deleted_at = $1 WHERE id = $2",
        &[
            Value::Text("2020-01-01T00:00:00.000000Z".into()),
            Value::Uuid(independent),
        ],
    )
    .await
    .expect("independent delete");

    User::soft_delete(&pool, user).await.expect("soft delete");
    User::restore(&pool, user).await.expect("restore");

    assert!(
        deleted_at(&pool, "posts", independent).await.is_some(),
        "independently deleted row must stay trashed"
    );
    assert!(
        deleted_at(&pool, "posts", cascaded).await.is_none(),
        "cascaded row must be restored"
    );
    pool.close().await;
}

/// Verifies a force delete cascades a hard delete to the children.
#[tokio::test]
async fn force_delete_cascades() {
    let _guard = test_lock().lock().await;
    let pool = pool().await;
    let user = Uuid::now_v7();
    let post = Uuid::now_v7();
    insert_user(&pool, user).await;
    insert_post(&pool, post, user).await;

    assert!(User::force_delete(&pool, user).await.expect("force delete"));

    assert!(!row_exists(&pool, "users", user).await);
    assert!(!row_exists(&pool, "posts", post).await);
    pool.close().await;
}

/// Verifies the derive emits the cascade overrides and the default stays off.
#[test]
fn derive_emits_cascade_overrides() {
    assert!(Ghost::cascade_soft_deletes());
    assert_eq!(Ghost::cascade_relations(), &["posts"]);
    assert!(!Plain::cascade_soft_deletes());
    assert!(Plain::cascade_relations().is_empty());
}

/// Verifies an undeclared cascade relation is a typed runtime error.
#[tokio::test]
async fn cascade_relation_not_declared_is_typed_error() {
    let _guard = test_lock().lock().await;
    let pool = pool().await;
    let ghost = Uuid::now_v7();
    let s = now();
    pool.execute_bind(
        "INSERT INTO ghosts (id, created_at, updated_at) VALUES ($1, $2, $3)",
        &[Value::Uuid(ghost), Value::Text(s.clone()), Value::Text(s)],
    )
    .await
    .expect("insert ghost");

    let error = Ghost::soft_delete(&pool, ghost)
        .await
        .expect_err("undeclared cascade relation must fail");
    match error {
        OrmError::InvalidState(message) => {
            assert!(message.contains("posts"), "{message}");
            assert!(message.contains("not declared"), "{message}");
        }
        other => panic!("expected InvalidState, got {other:?}"),
    }
    pool.close().await;
}

/// Verifies every parent/child event in one cascade shares the batch uuid.
#[tokio::test]
async fn activity_batch_uuid_shared() {
    let _guard = test_lock().lock().await;
    let pool = pool().await;
    let recorder = Arc::new(CapturingRecorder::default());
    register_activity_recorder(recorder.clone());

    let user = Uuid::now_v7();
    let (post_a, post_b) = (Uuid::now_v7(), Uuid::now_v7());
    insert_user(&pool, user).await;
    insert_post(&pool, post_a, user).await;
    insert_post(&pool, post_b, user).await;

    User::soft_delete(&pool, user).await.expect("soft delete");

    // The recorder is process-wide; scope assertions to this test's own rows.
    let events: Vec<ActivityEvent> = recorder
        .events()
        .into_iter()
        .filter(|event| {
            event.model_id.as_deref().is_some_and(|id| {
                id == user.to_string() || id == post_a.to_string() || id == post_b.to_string()
            })
        })
        .collect();
    // One parent delete + one child delete per post.
    assert_eq!(events.len(), 3, "parent + two children");
    assert_eq!(events[0].operation, ActivityOperation::Deleted);
    assert_eq!(events[0].model_type, "User");
    let batch = events[0].batch_uuid.clone().expect("batch uuid set");
    for event in &events {
        assert_eq!(event.operation, ActivityOperation::Deleted);
        assert_eq!(event.batch_uuid.as_deref(), Some(batch.as_str()));
    }
    let child_types: Vec<&str> = events[1..]
        .iter()
        .map(|event| event.model_type.as_str())
        .collect();
    assert!(child_types.iter().all(|name| *name == "posts"));

    clear_activity_recorder();
    pool.close().await;
}
