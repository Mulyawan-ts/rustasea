//! Composite-key relation eager-loading tests (ADOPT-017, compoships parity).
//!
//! Exercises `Relation::has_many_composite` / `belongs_to_composite` against a
//! real in-memory SQLite pool: composite parents resolve their related rows by
//! the full key tuple in a single batched query, so two parents sharing a
//! single-column value never conflate. Also covers the typed arity error and a
//! single-key regression check.

use rustasea_orm::{DbPool, OrmError, QueryBuilder, Relation, Value};
use serde_json::Value as JsonValue;
use uuid::Uuid;

/// Build a pool with composite-key tables.
///
/// `memberships` is the composite parent keyed by `(tenant_id, user_id)`;
/// `activity_logs` is its child carrying many rows per parent tuple. `tenants`
/// is a composite parent keyed by `(tenant_id, region)` and `tenant_links` is
/// its child carrying the composite foreign key onto `tenants`.
async fn pool_with_composite() -> DbPool {
    let pool = DbPool::connect("sqlite::memory:").await.unwrap();
    pool.execute_script(
        "CREATE TABLE memberships (
            tenant_id BLOB NOT NULL,
            user_id BLOB NOT NULL,
            role TEXT NOT NULL,
            PRIMARY KEY (tenant_id, user_id)
        );
        CREATE TABLE activity_logs (
            tenant_id BLOB NOT NULL,
            user_id BLOB NOT NULL,
            entry TEXT NOT NULL
        );
        CREATE TABLE tenants (
            tenant_id BLOB NOT NULL,
            region TEXT NOT NULL,
            name TEXT NOT NULL,
            PRIMARY KEY (tenant_id, region)
        );
        CREATE TABLE tenant_links (
            tenant_id BLOB NOT NULL,
            region TEXT NOT NULL,
            label TEXT NOT NULL
        );",
    )
    .await
    .unwrap();
    pool
}

/// Insert a membership (composite parent) keyed by `(tenant_id, user_id)`.
async fn insert_membership(pool: &DbPool, tenant_id: Uuid, user_id: Uuid, role: &str) {
    pool.execute_bind(
        "INSERT INTO memberships (tenant_id, user_id, role) VALUES ($1, $2, $3)",
        &[
            Value::Uuid(tenant_id),
            Value::Uuid(user_id),
            Value::Text(role.to_string()),
        ],
    )
    .await
    .unwrap();
}

/// Insert an activity log (composite child) keyed by `(tenant_id, user_id)`.
async fn insert_activity(pool: &DbPool, tenant_id: Uuid, user_id: Uuid, entry: &str) {
    pool.execute_bind(
        "INSERT INTO activity_logs (tenant_id, user_id, entry) VALUES ($1, $2, $3)",
        &[
            Value::Uuid(tenant_id),
            Value::Uuid(user_id),
            Value::Text(entry.to_string()),
        ],
    )
    .await
    .unwrap();
}

/// Insert a tenant (composite parent) keyed by `(tenant_id, region)`.
async fn insert_tenant(pool: &DbPool, tenant_id: Uuid, region: &str, name: &str) {
    pool.execute_bind(
        "INSERT INTO tenants (tenant_id, region, name) VALUES ($1, $2, $3)",
        &[
            Value::Uuid(tenant_id),
            Value::Text(region.to_string()),
            Value::Text(name.to_string()),
        ],
    )
    .await
    .unwrap();
}

/// Insert a link (composite child) carrying the FK onto `tenants`.
async fn insert_tenant_link(pool: &DbPool, tenant_id: Uuid, region: &str, label: &str) {
    pool.execute_bind(
        "INSERT INTO tenant_links (tenant_id, region, label) VALUES ($1, $2, $3)",
        &[
            Value::Uuid(tenant_id),
            Value::Text(region.to_string()),
            Value::Text(label.to_string()),
        ],
    )
    .await
    .unwrap();
}

/// Verifies a composite `HasMany` groups multiple related rows per key tuple.
#[tokio::test]
async fn composite_has_many_groups_by_tuple() {
    let pool = pool_with_composite().await;
    let tenant = Uuid::now_v7();
    let alice = Uuid::now_v7();
    let bob = Uuid::now_v7();
    insert_membership(&pool, tenant, alice, "admin").await;
    insert_membership(&pool, tenant, bob, "member").await;
    // Two logs for alice, one for bob — the tuple partitions them.
    insert_activity(&pool, tenant, alice, "login").await;
    insert_activity(&pool, tenant, alice, "logout").await;
    insert_activity(&pool, tenant, bob, "login").await;

    let relation = Relation::has_many_composite(
        "activity_logs",
        "activity_logs",
        &["tenant_id", "user_id"],
        &["tenant_id", "user_id"],
    )
    .unwrap();
    let rows = QueryBuilder::table("memberships")
        .with_relations(vec![relation])
        .with(&["activity_logs"])
        .get_eager(&pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    let by_role = |role: &str| {
        rows.iter()
            .find(|row| row["role"] == role)
            .expect("membership present")
    };
    assert_eq!(
        by_role("admin")["relations"]["activity_logs"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        by_role("member")["relations"]["activity_logs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

/// Acceptance: two tenants sharing a user id resolve DISTINCT composite
/// relations — a single-column key would conflate them.
#[tokio::test]
async fn composite_has_many_disambiguates_shared_user_id() {
    let pool = pool_with_composite().await;
    let tenant_a = Uuid::now_v7();
    let tenant_b = Uuid::now_v7();
    let shared_user = Uuid::now_v7();
    insert_membership(&pool, tenant_a, shared_user, "admin-a").await;
    insert_membership(&pool, tenant_b, shared_user, "admin-b").await;
    // One distinct log per tenant, both for the shared user id.
    insert_activity(&pool, tenant_a, shared_user, "entry-a").await;
    insert_activity(&pool, tenant_b, shared_user, "entry-b").await;

    let relation = Relation::has_many_composite(
        "activity_logs",
        "activity_logs",
        &["tenant_id", "user_id"],
        &["tenant_id", "user_id"],
    )
    .unwrap();
    let rows = QueryBuilder::table("memberships")
        .with_relations(vec![relation])
        .with(&["activity_logs"])
        .get_eager(&pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);

    let by_role = |role: &str| {
        rows.iter()
            .find(|row| row["role"] == role)
            .expect("membership present")
    };
    // The shared user id must NOT bleed: tenant-a's membership sees only
    // tenant-a's log, tenant-b's only tenant-b's.
    assert_eq!(
        by_role("admin-a")["relations"]["activity_logs"][0]["entry"],
        "entry-a"
    );
    assert_eq!(
        by_role("admin-b")["relations"]["activity_logs"][0]["entry"],
        "entry-b"
    );
}

/// Verifies a composite `BelongsTo` matches the parent by its local-key tuple.
#[tokio::test]
async fn composite_belongs_to_matches_parent_tuple() {
    let pool = pool_with_composite().await;
    let tenant = Uuid::now_v7();
    insert_tenant(&pool, tenant, "eu", "Acme EU").await;
    insert_tenant(&pool, tenant, "us", "Acme US").await;
    insert_tenant_link(&pool, tenant, "eu", "link-eu").await;
    insert_tenant_link(&pool, tenant, "us", "link-us").await;

    // Child foreign keys `(tenant_id, region)` match the parent's local keys
    // `(tenant_id, region)` — the region disambiguates the shared tenant id.
    let relation = Relation::belongs_to_composite(
        "tenant",
        "tenants",
        &["tenant_id", "region"],
        &["tenant_id", "region"],
    )
    .unwrap();
    let rows = QueryBuilder::table("tenant_links")
        .with_relations(vec![relation])
        .with(&["tenant"])
        .get_eager(&pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    let by_label = |label: &str| {
        rows.iter()
            .find(|row| row["label"] == label)
            .expect("link present")
    };
    assert_eq!(
        by_label("link-eu")["relations"]["tenant"]["name"],
        "Acme EU"
    );
    assert_eq!(
        by_label("link-us")["relations"]["tenant"]["name"],
        "Acme US"
    );
}

/// Verifies a composite `BelongsTo` with a missing parent attaches null.
#[tokio::test]
async fn composite_belongs_to_missing_parent_is_null() {
    let pool = pool_with_composite().await;
    let tenant = Uuid::now_v7();
    insert_tenant_link(&pool, tenant, "eu", "orphan").await;

    let relation = Relation::belongs_to_composite(
        "tenant",
        "tenants",
        &["tenant_id", "region"],
        &["tenant_id", "region"],
    )
    .unwrap();
    let rows = QueryBuilder::table("tenant_links")
        .with_relations(vec![relation])
        .with(&["tenant"])
        .get_eager(&pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["relations"]["tenant"], JsonValue::Null);
}

/// Verifies a composite `HasMany` over an empty parent set stays empty.
#[tokio::test]
async fn composite_has_many_empty_is_array() {
    let pool = pool_with_composite().await;
    let relation = Relation::has_many_composite(
        "activity_logs",
        "activity_logs",
        &["tenant_id", "user_id"],
        &["tenant_id", "user_id"],
    )
    .unwrap();
    let rows = QueryBuilder::table("memberships")
        .with_relations(vec![relation])
        .with(&["activity_logs"])
        .get_eager(&pool)
        .await
        .unwrap();
    assert!(rows.is_empty());
}

/// Verifies a mismatched composite key declaration is a typed error.
#[test]
fn composite_relation_mismatched_arity_is_typed_error() {
    let error = Relation::has_many_composite("x", "xs", &["a", "b"], &["c"])
        .expect_err("arity mismatch must fail");
    assert!(matches!(error, OrmError::InvalidState(_)), "got {error:?}");
}

/// Verifies an empty composite key declaration is a typed error.
#[test]
fn composite_relation_empty_keys_is_typed_error() {
    let error =
        Relation::has_many_composite("x", "xs", &[], &[]).expect_err("empty keys must fail");
    assert!(matches!(error, OrmError::InvalidState(_)), "got {error:?}");
}

/// Regression: single-key relations still resolve after the composite branch
/// was added to the eager loader.
#[tokio::test]
async fn single_key_relations_still_work() {
    let pool = DbPool::connect("sqlite::memory:").await.unwrap();
    pool.execute_script(
        "CREATE TABLE users (id BLOB NOT NULL, name TEXT NOT NULL, PRIMARY KEY (id));
         CREATE TABLE posts (id BLOB NOT NULL, user_id BLOB NOT NULL, title TEXT NOT NULL);",
    )
    .await
    .unwrap();
    let ada = Uuid::now_v7();
    pool.execute_bind(
        "INSERT INTO users (id, name) VALUES ($1, $2)",
        &[Value::Uuid(ada), Value::Text("Ada".into())],
    )
    .await
    .unwrap();
    pool.execute_bind(
        "INSERT INTO posts (id, user_id, title) VALUES ($1, $2, $3)",
        &[
            Value::Uuid(Uuid::now_v7()),
            Value::Uuid(ada),
            Value::Text("First".into()),
        ],
    )
    .await
    .unwrap();

    let relation = Relation::has_many("posts", "posts", "User");
    assert!(!relation.is_composite());
    let rows = QueryBuilder::table("users")
        .with_relations(vec![relation])
        .with(&["posts"])
        .get_eager(&pool)
        .await
        .unwrap();
    assert_eq!(rows[0]["relations"]["posts"].as_array().unwrap().len(), 1);
}
