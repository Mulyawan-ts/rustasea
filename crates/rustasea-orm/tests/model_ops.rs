//! Async execution round-trip tests (M2-B).
//!
//! Exercises the `QueryBuilder` and `ModelOps` executors against a real
//! in-memory SQLite pool: create → first → update → delete (soft) → force
//! delete, `first_or_fail` NotFound, pagination envelope and `chunk_by`
//! windowing. The sqlx runtime API is used throughout.

use chrono::{DateTime, Utc};
use rustasea_orm::{DbPool, Model, ModelOps, OrmError, QueryBuilder, Value};
use uuid::Uuid;

/// Derived model mapped to `users`, with tracked timestamps and soft deletes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, rustasea_macros::Model)]
struct User {
    id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

/// Build a fresh in-memory pool with the `users` table applied.
async fn pool_with_users() -> DbPool {
    let pool = DbPool::connect("sqlite::memory:").await.unwrap();
    pool.execute_bind(
        "CREATE TABLE users (
            id BLOB PRIMARY KEY,
            name TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            deleted_at TEXT
        )",
        &[],
    )
    .await
    .unwrap();
    pool
}

/// Build an unsaved `User` instance with a fresh id.
fn sample(name: &str) -> User {
    let now = Utc::now();
    User {
        id: Uuid::now_v7(),
        name: name.to_string(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
    }
}

/// Verifies create → first → update round-trips against SQLite.
#[tokio::test]
async fn create_first_update_round_trip() {
    let pool = pool_with_users().await;
    let created = User::create(&pool, sample("Ada")).await.unwrap();
    assert_eq!(created.name, "Ada");

    let fetched = <User as Model>::query()
        .where_eq("name", "Ada")
        .first(&pool)
        .await
        .unwrap()
        .expect("row should exist");
    let fetched: User = serde_json::from_value(fetched).unwrap();
    assert_eq!(fetched.id, created.id);

    let mut updated = fetched;
    updated.name = "Ada Lovelace".to_string();
    let persisted = User::update(&pool, updated).await.unwrap();
    assert_eq!(persisted.name, "Ada Lovelace");
}

/// Verifies `first_or_fail` returns `NotFound` when no row matches.
#[tokio::test]
async fn first_or_fail_missing_returns_not_found() {
    let pool = pool_with_users().await;
    let err = <User as Model>::query()
        .where_eq("name", "nobody")
        .first_or_fail(&pool)
        .await
        .unwrap_err();
    assert!(matches!(err, OrmError::NotFound));
}

/// Verifies soft delete excludes the row and force delete removes it.
#[tokio::test]
async fn soft_delete_excludes_and_force_delete_removes() {
    let pool = pool_with_users().await;
    let created = User::create(&pool, sample("Grace")).await.unwrap();

    assert!(User::delete(&pool, created.id).await.unwrap());
    let active = <User as Model>::query()
        .where_key(created.id)
        .first(&pool)
        .await
        .unwrap();
    assert!(active.is_none(), "soft-deleted row must be excluded");

    let trashed = <User as Model>::query_with_trashed()
        .where_key(created.id)
        .first(&pool)
        .await
        .unwrap();
    assert!(trashed.is_some(), "row still present with deleted_at set");

    assert!(User::force_delete(&pool, created.id).await.unwrap());
    let gone = <User as Model>::query_with_trashed()
        .where_key(created.id)
        .first(&pool)
        .await
        .unwrap();
    assert!(gone.is_none());
}

/// Verifies paginate fills items, metadata and navigation links.
#[tokio::test]
async fn paginate_fills_data_meta_and_links() {
    let pool = pool_with_users().await;
    for index in 0..5 {
        User::create(&pool, sample(&format!("user-{index}")))
            .await
            .unwrap();
    }

    let page = QueryBuilder::table("users")
        .paginate(&pool, 2, 2)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.total, 5);
    assert_eq!(page.last_page, 3);
    assert_eq!(page.current_page, 2);
    assert_eq!(page.links.prev.as_deref(), Some("?page=1"));
    assert_eq!(page.links.next.as_deref(), Some("?page=3"));

    let json = serde_json::to_value(&page).unwrap();
    assert!(json.get("data").is_some());
    assert!(json.get("meta").is_some());
    assert!(json.get("links").is_some());
}

/// Verifies chunk_by drives the callback once per non-empty window.
#[tokio::test]
async fn chunk_by_invokes_callback_per_window() {
    let pool = pool_with_users().await;
    for index in 0..5 {
        User::create(&pool, sample(&format!("user-{index}")))
            .await
            .unwrap();
    }

    let mut windows = 0usize;
    let mut rows = 0usize;
    let processed = QueryBuilder::table("users")
        .chunk_by(&pool, 2, |chunk| {
            windows += 1;
            rows += chunk.len();
            Ok(())
        })
        .await
        .unwrap();

    assert_eq!(windows, 3);
    assert_eq!(rows, 5);
    assert_eq!(processed, 5);
}

/// Verifies exists reports presence without loading rows.
#[tokio::test]
async fn exists_reports_presence() {
    let pool = pool_with_users().await;
    assert!(!QueryBuilder::table("users").exists(&pool).await.unwrap());
    User::create(&pool, sample("Alan")).await.unwrap();
    assert!(QueryBuilder::table("users").exists(&pool).await.unwrap());
    assert_eq!(QueryBuilder::table("users").count(&pool).await.unwrap(), 1);
}

/// Verifies `first_for_update` is gated on the **runtime** pool dialect.
///
/// An SQLite pool must surface the typed [`OrmError::UnsupportedDriver`] before
/// any round-trip — even when the `postgres` (or `mysql`) feature is compiled
/// in — rather than emitting `FOR UPDATE` for SQLite to reject at execution
/// time. This is the regression for the compile-time lock leak.
#[tokio::test]
async fn first_for_update_rejects_sqlite_pool_at_runtime() {
    let pool = pool_with_users().await;
    let id = Uuid::now_v7();

    let error = <User as ModelOps>::first_for_update(&pool, id)
        .await
        .expect_err("sqlite has no row locks");
    assert!(
        matches!(error, OrmError::UnsupportedDriver(_)),
        "expected UnsupportedDriver, got {error:?}"
    );
}

/// Composite-primary-key model: no `id` field, key declared via `#[model]`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, rustasea_macros::Model)]
#[model(table = "memberships", primary_key = ["tenant_id", "user_id"])]
struct Membership {
    tenant_id: Uuid,
    user_id: Uuid,
    role: String,
}

/// Build a fresh in-memory pool with the composite `memberships` table applied.
async fn pool_with_memberships() -> DbPool {
    let pool = DbPool::connect("sqlite::memory:").await.unwrap();
    pool.execute_bind(
        "CREATE TABLE memberships (
            tenant_id BLOB NOT NULL,
            user_id BLOB NOT NULL,
            role TEXT NOT NULL,
            PRIMARY KEY (tenant_id, user_id)
        )",
        &[],
    )
    .await
    .unwrap();
    pool
}

/// Verifies a composite-PK model round-trips create → refresh → update → delete.
#[tokio::test]
async fn composite_primary_key_round_trip() {
    let pool = pool_with_memberships().await;
    let tenant = Uuid::now_v7();
    let user = Uuid::now_v7();
    let membership = Membership {
        tenant_id: tenant,
        user_id: user,
        role: "admin".into(),
    };

    let created = Membership::create(&pool, membership).await.unwrap();
    assert_eq!(created.role, "admin");
    assert_eq!(created.primary_key_values().len(), 2);

    // Refresh addresses the row by its composite key tuple.
    let key = [Value::Uuid(tenant), Value::Uuid(user)];
    let fetched = Membership::refresh_by_key(&pool, &key)
        .await
        .unwrap()
        .expect("row should exist");
    assert_eq!(fetched.role, "admin");

    // Update the non-key column, keyed by the tuple.
    let mut updated = fetched;
    updated.role = "owner".into();
    let persisted = Membership::update(&pool, updated).await.unwrap();
    assert_eq!(persisted.role, "owner");

    // A second row for a DIFFERENT user under the same tenant is unaffected.
    let other = Membership {
        tenant_id: tenant,
        user_id: Uuid::now_v7(),
        role: "member".into(),
    };
    Membership::create(&pool, other).await.unwrap();
    assert_eq!(
        QueryBuilder::table("memberships")
            .count(&pool)
            .await
            .unwrap(),
        2
    );

    // Delete by composite key removes exactly the addressed row.
    assert!(Membership::delete_by_key(&pool, &key).await.unwrap());
    assert_eq!(
        QueryBuilder::table("memberships")
            .count(&pool)
            .await
            .unwrap(),
        1
    );
    assert!(Membership::refresh_by_key(&pool, &key)
        .await
        .unwrap()
        .is_none());
}

/// Verifies a composite key tuple of the wrong arity is a typed error.
#[tokio::test]
async fn composite_key_arity_mismatch_is_typed_error() {
    let pool = pool_with_memberships().await;
    let error = Membership::refresh_by_key(&pool, &[Value::Uuid(Uuid::now_v7())])
        .await
        .expect_err("partial composite key must fail");
    assert!(
        matches!(error, OrmError::InvalidState(_)),
        "expected InvalidState, got {error:?}"
    );
}
