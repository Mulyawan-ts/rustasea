//! Sibling-relation and self-reference cascade tests (ADOPT-018).
//!
//! Split out of [`super`] to keep `cascade_soft_deletes.rs` within the 500-line
//! cap. Both tests share the parent's fixtures (`pool`, `insert_*`, `deleted_at`,
//! `test_lock`) through `use super::*;`.

use super::*;

/// Insert a post row matched only by the `archived_posts` relation.
///
/// `owner_id` is a *different* user (the NOT NULL `user_id` column), while
/// `archived_by` points at the user under test — so the row is reachable only
/// through the `archived_posts` FK, not the `posts` one.
async fn insert_archived_post(pool: &DbPool, id: Uuid, owner_id: Uuid, archived_by: Uuid) {
    let s = now();
    let sql = "INSERT INTO posts (id, user_id, archived_by, title, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6)";
    let values = [
        Value::Uuid(id),
        Value::Uuid(owner_id),
        Value::Uuid(archived_by),
        Value::Text("archived".into()),
        Value::Text(s.clone()),
        Value::Text(s),
    ];
    pool.execute_bind(sql, &values)
        .await
        .expect("insert archived post");
}

/// Verifies two sibling relations on the same table both cascade.
///
/// `posts` and `archived_posts` target the same `posts` table through different
/// foreign keys (`user_id` vs `archived_by`). The visited set keys by relation
/// name, so both fan-outs run on one delete rather than the second being
/// skipped because its child table was already seen.
#[tokio::test]
async fn sibling_relations_on_same_table_both_cascade() {
    let _guard = test_lock().lock().await;
    let pool = pool().await;
    let user = Uuid::now_v7();
    let other = Uuid::now_v7();
    let owned = Uuid::now_v7();
    let archived = Uuid::now_v7();
    insert_user(&pool, user).await;
    insert_user(&pool, other).await;
    insert_post(&pool, owned, user).await;
    insert_archived_post(&pool, archived, other, user).await;

    User::soft_delete(&pool, user).await.expect("soft delete");

    assert!(
        deleted_at(&pool, "posts", owned).await.is_some(),
        "the `posts` relation cascades on user_id"
    );
    assert!(
        deleted_at(&pool, "posts", archived).await.is_some(),
        "the `archived_posts` relation must still cascade on archived_by"
    );
    pool.close().await;
}

/// Verifies a self-referential cascade is guarded.
///
/// A relation pointing back at the parent's own table is skipped, so the parent
/// never cascades into its own rows (and never reaches the child).
#[tokio::test]
async fn self_referential_cascade_is_guarded() {
    let _guard = test_lock().lock().await;
    let pool = pool().await;
    let parent = Uuid::now_v7();
    let child = Uuid::now_v7();
    insert_category(&pool, parent, None).await;
    insert_category(&pool, child, Some(parent)).await;

    Category::soft_delete(&pool, parent)
        .await
        .expect("soft delete");

    assert!(deleted_at(&pool, "categories", parent).await.is_some());
    assert!(
        deleted_at(&pool, "categories", child).await.is_none(),
        "the self-referential guard prevents the fan-out"
    );
    pool.close().await;
}
