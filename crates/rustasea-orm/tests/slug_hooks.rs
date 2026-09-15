//! Slug-generation write-hook integration tests (ADOPT-016).
//!
//! Exercises the ORM write path against a real in-memory SQLite pool for models
//! that opt into `#[sluggable]`: create derives a unique slug from the source
//! column, collisions suffix deterministically (`-2`, `-3`, …), an empty source
//! is a typed error rather than a silent empty slug, `on_update` regenerates only
//! when the source changed, `find_by_slug` round-trips (and excludes trashed
//! rows), and a soft-deleted row's slug is never reused.

use chrono::{DateTime, Utc};
use rustasea_orm::{DbPool, ModelOps, OrmError, SluggableFind};
use uuid::Uuid;

/// A model that derives its slug from `name`, never regenerating on update.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, rustasea_macros::Model)]
#[sluggable(source = "name")]
struct Post {
    id: Uuid,
    name: String,
    slug: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

/// A model that regenerates its slug when `name` changes on update.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, rustasea_macros::Model)]
#[sluggable(source = "name", on_update = "true")]
struct Article {
    id: Uuid,
    name: String,
    slug: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

/// Build a fresh in-memory pool with the `posts` and `articles` tables applied.
async fn pool() -> DbPool {
    let pool = DbPool::connect("sqlite::memory:").await.expect("connect");
    for table in ["posts", "articles"] {
        let sql = format!(
            "CREATE TABLE {table} (
                id BLOB PRIMARY KEY,
                name TEXT NOT NULL,
                slug TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                deleted_at TEXT
            )"
        );
        pool.execute_bind(&sql, &[]).await.expect("create table");
    }
    pool
}

/// An unsaved `Post` with the given source name.
fn post(name: &str) -> Post {
    let now = Utc::now();
    Post {
        id: Uuid::now_v7(),
        name: name.to_string(),
        slug: String::new(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
    }
}

/// An unsaved `Article` with the given source name.
fn article(name: &str) -> Article {
    let now = Utc::now();
    Article {
        id: Uuid::now_v7(),
        name: name.to_string(),
        slug: String::new(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
    }
}

/// Create derives a URL-safe slug from the source column.
#[tokio::test]
async fn create_generates_slug_from_source() {
    let pool = pool().await;
    let created = Post::create(&pool, post("Hello World"))
        .await
        .expect("create");
    assert_eq!(created.slug, "hello-world");

    let fetched = Post::find_by_slug(&pool, "hello-world")
        .await
        .expect("query")
        .expect("row found by slug");
    assert_eq!(fetched.id, created.id);
    pool.close().await;
}

/// Two rows with the same source get distinct, deterministic slugs.
#[tokio::test]
async fn collisions_are_suffixed_deterministically() {
    let pool = pool().await;
    let first = Post::create(&pool, post("Ada Lovelace"))
        .await
        .expect("create");
    let second = Post::create(&pool, post("Ada Lovelace"))
        .await
        .expect("create");
    let third = Post::create(&pool, post("Ada Lovelace"))
        .await
        .expect("create");

    assert_eq!(first.slug, "ada-lovelace");
    assert_eq!(second.slug, "ada-lovelace-2");
    assert_eq!(third.slug, "ada-lovelace-3");
    pool.close().await;
}

/// An empty source column is a typed error, never a silent empty slug.
#[tokio::test]
async fn empty_source_is_a_typed_error() {
    let pool = pool().await;
    let error = Post::create(&pool, post("   "))
        .await
        .expect_err("empty source must fail");
    assert!(matches!(error, OrmError::Slug(_)), "got {error:?}");

    // Nothing was persisted.
    assert!(Post::find_by_slug(&pool, "")
        .await
        .expect("query")
        .is_none());
    pool.close().await;
}

/// Without `on_update`, changing the source leaves the slug stable.
#[tokio::test]
async fn update_without_on_update_keeps_slug() {
    let pool = pool().await;
    let created = Post::create(&pool, post("Original")).await.expect("create");
    assert_eq!(created.slug, "original");

    let mut changed = created.clone();
    changed.name = "Renamed".to_string();
    let persisted = Post::update(&pool, changed).await.expect("update");
    assert_eq!(persisted.slug, "original");
    assert_eq!(persisted.name, "Renamed");
    pool.close().await;
}

/// With `on_update`, a changed source regenerates the slug.
#[tokio::test]
async fn update_with_on_update_regenerates_when_source_changed() {
    let pool = pool().await;
    let created = Article::create(&pool, article("First Draft"))
        .await
        .expect("create");
    assert_eq!(created.slug, "first-draft");

    let mut changed = created.clone();
    changed.name = "Second Draft".to_string();
    let persisted = Article::update(&pool, changed).await.expect("update");
    assert_eq!(persisted.slug, "second-draft");
    pool.close().await;
}

/// With `on_update`, an unchanged source leaves the slug stable (no self-collision).
#[tokio::test]
async fn update_with_on_update_keeps_slug_when_source_unchanged() {
    let pool = pool().await;
    let created = Article::create(&pool, article("Stable"))
        .await
        .expect("create");

    // Change an unrelated column only — the source is identical.
    let mut unchanged = created.clone();
    let persisted = Article::update(&pool, unchanged.clone())
        .await
        .expect("update");
    assert_eq!(persisted.slug, "stable");

    unchanged.name = "Stable".to_string();
    let persisted = Article::update(&pool, unchanged).await.expect("update");
    assert_eq!(persisted.slug, "stable");
    pool.close().await;
}

/// `find_by_slug_or_fail` surfaces `NotFound` for a missing slug.
#[tokio::test]
async fn find_by_slug_or_fail_missing_returns_not_found() {
    let pool = pool().await;
    let error = Post::find_by_slug_or_fail(&pool, "ghost")
        .await
        .expect_err("missing slug must fail");
    assert!(matches!(error, OrmError::NotFound));
    pool.close().await;
}

/// A soft-deleted row keeps its slug and is excluded from `find_by_slug`.
#[tokio::test]
async fn soft_deleted_row_keeps_slug_and_is_excluded() {
    let pool = pool().await;
    let created = Post::create(&pool, post("Unique Name"))
        .await
        .expect("create");
    assert_eq!(created.slug, "unique-name");

    assert!(Post::delete(&pool, created.id).await.expect("soft delete"));

    // The trashed row is invisible to the route-key lookup …
    assert!(Post::find_by_slug(&pool, "unique-name")
        .await
        .expect("query")
        .is_none());

    // … but its slug is never reused: the next row suffixes to `-2`.
    let next = Post::create(&pool, post("Unique Name"))
        .await
        .expect("create");
    assert_eq!(next.slug, "unique-name-2");
    pool.close().await;
}
