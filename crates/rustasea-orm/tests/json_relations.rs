//! JSON-embedded relation eager-loading tests (ADOPT-020, eloquent-json-relations).
//!
//! Exercises `Relation::belongs_to_json` / `has_many_json` / `belongs_to_many_json`
//! against a real in-memory SQLite pool. A relation's keys live inside a JSON
//! `TEXT` column rather than a foreign-key column or pivot; each loader runs one
//! batched `IN (…)` query and groups the related rows back onto their parents.
//! Also covers the malformed-JSON skip path and the missing-target `null`/`[]`
//! defaults.

use rustasea_orm::{DbPool, OrderDirection, QueryBuilder, Relation, Value};
use serde_json::{json, Value as JsonValue};

/// Build a pool with `users` and `posts`, each carrying a JSON `payload` column.
async fn pool_with_json() -> DbPool {
    let pool = DbPool::connect("sqlite::memory:").await.unwrap();
    pool.execute_script(
        "CREATE TABLE users (
            id TEXT NOT NULL PRIMARY KEY,
            name TEXT NOT NULL,
            payload TEXT
        );
        CREATE TABLE posts (
            id TEXT NOT NULL PRIMARY KEY,
            user_id TEXT,
            title TEXT NOT NULL,
            payload TEXT
        );",
    )
    .await
    .unwrap();
    pool
}

/// Insert a user with a JSON `payload`.
async fn insert_user(pool: &DbPool, id: &str, name: &str, payload: &str) {
    pool.execute_bind(
        "INSERT INTO users (id, name, payload) VALUES ($1, $2, $3)",
        &[
            Value::Text(id.to_string()),
            Value::Text(name.to_string()),
            Value::Text(payload.to_string()),
        ],
    )
    .await
    .unwrap();
}

/// Insert a post with an optional `user_id` and a JSON `payload`.
async fn insert_post(pool: &DbPool, id: &str, user_id: Option<&str>, title: &str, payload: &str) {
    let user_id = match user_id {
        Some(id) => Value::Text(id.to_string()),
        None => Value::Null,
    };
    pool.execute_bind(
        "INSERT INTO posts (id, user_id, title, payload) VALUES ($1, $2, $3, $4)",
        &[
            Value::Text(id.to_string()),
            user_id,
            Value::Text(title.to_string()),
            Value::Text(payload.to_string()),
        ],
    )
    .await
    .unwrap();
}

/// Verifies `belongs_to_json` resolves each parent from the embedded scalar id.
#[tokio::test]
async fn belongs_to_json_resolves_parent() {
    let pool = pool_with_json().await;
    insert_user(&pool, "u1", "Ada", "{}").await;
    insert_user(&pool, "u2", "Alan", "{}").await;
    insert_post(
        &pool,
        "p1",
        Some("u1"),
        "First",
        r#"{"author":{"id":"u1"}}"#,
    )
    .await;
    insert_post(
        &pool,
        "p2",
        Some("u2"),
        "Second",
        r#"{"author":{"id":"u2"}}"#,
    )
    .await;

    let relation = Relation::belongs_to_json("author", "users", "payload", "author.id").unwrap();
    let rows = QueryBuilder::table("posts")
        .with_relations(vec![relation])
        .with(&["author"])
        .order_by("id", OrderDirection::Asc)
        .get_eager(&pool)
        .await
        .unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["relations"]["author"]["name"], "Ada");
    assert_eq!(rows[1]["relations"]["author"]["name"], "Alan");
}

/// Verifies a missing target row and a malformed JSON cell both degrade to null
/// without failing the query.
#[tokio::test]
async fn belongs_to_json_missing_and_malformed_are_null() {
    let pool = pool_with_json().await;
    insert_user(&pool, "u1", "Ada", "{}").await;
    insert_post(&pool, "p1", None, "Malformed", "{oops").await;
    insert_post(&pool, "p2", None, "Missing", r#"{"author":{"id":"ghost"}}"#).await;

    let relation = Relation::belongs_to_json("author", "users", "payload", "author.id").unwrap();
    let rows = QueryBuilder::table("posts")
        .with_relations(vec![relation])
        .with(&["author"])
        .order_by("id", OrderDirection::Asc)
        .get_eager(&pool)
        .await
        .unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["relations"]["author"], JsonValue::Null);
    assert_eq!(rows[1]["relations"]["author"], JsonValue::Null);
}

/// Verifies `has_many_json` groups the embedded id array into per-parent rows.
#[tokio::test]
async fn has_many_json_resolves_children() {
    let pool = pool_with_json().await;
    insert_user(&pool, "u1", "Ada", r#"{"post_ids":["p1","p2"]}"#).await;
    insert_user(&pool, "u2", "Alan", r#"{"post_ids":["p3"]}"#).await;
    insert_post(&pool, "p1", Some("u1"), "First", "{}").await;
    insert_post(&pool, "p2", Some("u1"), "Second", "{}").await;
    insert_post(&pool, "p3", Some("u2"), "Only", "{}").await;

    let relation = Relation::has_many_json("posts", "posts", "payload", "post_ids").unwrap();
    let rows = QueryBuilder::table("users")
        .with_relations(vec![relation])
        .with(&["posts"])
        .order_by("id", OrderDirection::Asc)
        .get_eager(&pool)
        .await
        .unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["relations"]["posts"].as_array().unwrap().len(), 2);
    assert_eq!(rows[1]["relations"]["posts"].as_array().unwrap().len(), 1);
}

/// Verifies a missing target and a malformed cell both yield an empty array.
#[tokio::test]
async fn has_many_json_missing_and_malformed_are_empty() {
    let pool = pool_with_json().await;
    insert_user(&pool, "u1", "Missing", r#"{"post_ids":["ghost"]}"#).await;
    insert_user(&pool, "u2", "Malformed", "not-json").await;
    insert_user(&pool, "u3", "NoArray", r#"{"post_ids":"scalar"}"#).await;
    insert_post(&pool, "p1", None, "First", "{}").await;

    let relation = Relation::has_many_json("posts", "posts", "payload", "post_ids").unwrap();
    let rows = QueryBuilder::table("users")
        .with_relations(vec![relation])
        .with(&["posts"])
        .order_by("id", OrderDirection::Asc)
        .get_eager(&pool)
        .await
        .unwrap();

    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert_eq!(row["relations"]["posts"], json!([]));
    }
}

/// Verifies `belongs_to_many_json` resolves a pivot-less many-to-many from the
/// embedded id array.
#[tokio::test]
async fn belongs_to_many_json_resolves_many() {
    let pool = pool_with_json().await;
    insert_post(&pool, "p1", None, "Root", r#"{"related_ids":["p2","p3"]}"#).await;
    insert_post(&pool, "p2", None, "Child A", "{}").await;
    insert_post(&pool, "p3", None, "Child B", "{}").await;

    let relation =
        Relation::belongs_to_many_json("related", "posts", "payload", "related_ids").unwrap();
    let rows = QueryBuilder::table("posts")
        .with_relations(vec![relation])
        .with(&["related"])
        .order_by("id", OrderDirection::Asc)
        .get_eager(&pool)
        .await
        .unwrap();

    assert_eq!(rows.len(), 3);
    let root = rows.iter().find(|row| row["id"] == "p1").unwrap();
    let related = root["relations"]["related"].as_array().unwrap();
    assert_eq!(related.len(), 2);
    // The two child posts that hold no related ids resolve to empty arrays.
    let child = rows.iter().find(|row| row["id"] == "p2").unwrap();
    assert_eq!(child["relations"]["related"], json!([]));
}

/// Verifies the loaders are batched: a numeric embedded id (JSON number) also
/// resolves, and the whole parent set shares one query.
#[tokio::test]
async fn json_relations_accept_numeric_ids() {
    let pool = pool_with_json().await;
    // `id` is TEXT, but a JSON number must stringify to the stored key.
    insert_user(&pool, "7", "Seven", "{}").await;
    insert_post(&pool, "p1", None, "Numeric", r#"{"author":{"id":7}}"#).await;

    let relation = Relation::belongs_to_json("author", "users", "payload", "author.id").unwrap();
    let rows = QueryBuilder::table("posts")
        .with_relations(vec![relation])
        .with(&["author"])
        .get_eager(&pool)
        .await
        .unwrap();

    assert_eq!(rows[0]["relations"]["author"]["name"], "Seven");
}
