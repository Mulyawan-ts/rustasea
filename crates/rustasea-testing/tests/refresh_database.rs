//! Integration tests for [`RefreshDatabase`] transaction rollback.
//!
//! These exercise the Laravel-parity contract against an in-memory SQLite pool
//! (no docker required): a test body runs inside a transaction that is rolled
//! back afterwards, so the next test observes a pristine table without the
//! schema being recreated. Written but **not executed** here — the tester owns
//! execution.

use rustasea_orm::{DbPool, Migration, Migrator, Result as OrmResult, Value};
use rustasea_testing::{migrate_once, RefreshDatabase};

/// A pristine in-memory pool with a `widgets` table and no rows.
///
/// In-memory SQLite is pinned to a single pooled connection, so the table
/// created here survives across guard begin/rollback cycles on the same pool.
async fn memory_pool() -> DbPool {
    let pool = DbPool::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    pool.execute_script("CREATE TABLE widgets (id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
        .await
        .expect("create widgets");
    pool
}

/// Counts rows currently visible in `widgets` on the pool.
async fn widget_count(pool: &DbPool) -> usize {
    pool.fetch_json("SELECT id FROM widgets", &[])
        .await
        .expect("count widgets")
        .len()
}

/// Positive: a write inside the guard is discarded by an explicit rollback.
#[tokio::test]
async fn rollback_discards_writes() {
    let pool = memory_pool().await;

    let mut guard = RefreshDatabase::begin(&pool).await.expect("begin");
    assert!(guard.is_open());
    guard
        .execute_bind(
            "INSERT INTO widgets (id, name) VALUES ($1, $2)",
            &[Value::Int(1), Value::Text("gadget".into())],
        )
        .await
        .expect("insert inside guard");
    guard.rollback().await.expect("rollback");

    assert_eq!(widget_count(&pool).await, 0, "table must be pristine");
    pool.close().await;
}

/// Positive: a fresh guard (the "next test") sees no rows from the prior guard.
#[tokio::test]
async fn subsequent_guard_observes_pristine_state() {
    let pool = memory_pool().await;

    let mut first = RefreshDatabase::begin(&pool).await.expect("begin first");
    first
        .execute_bind(
            "INSERT INTO widgets (id, name) VALUES ($1, $2)",
            &[Value::Int(7), Value::Text("first".into())],
        )
        .await
        .expect("insert in first guard");
    first.rollback().await.expect("rollback first");

    let mut second = RefreshDatabase::begin(&pool).await.expect("begin second");
    let rows = second
        .transaction()
        .fetch_json("SELECT id FROM widgets", &[])
        .await
        .expect("select in second guard");
    assert!(rows.is_empty(), "second guard must start pristine");
    second.rollback().await.expect("rollback second");

    pool.close().await;
}

/// Positive: schema created by `migrate_once` survives rollback (created once).
#[tokio::test]
async fn migrated_schema_survives_rollback() {
    let pool = DbPool::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");

    struct CreateWidgets;
    impl Migration for CreateWidgets {
        fn name(&self) -> &str {
            "0001_create_widgets_table"
        }
        fn up(&self) -> OrmResult<String> {
            Ok("CREATE TABLE widgets (id INTEGER PRIMARY KEY, name TEXT NOT NULL);".into())
        }
        fn down(&self) -> OrmResult<String> {
            Ok("DROP TABLE widgets;".into())
        }
    }

    let mut migrator = Migrator::new();
    migrator.add(CreateWidgets);

    let mut guard = RefreshDatabase::begin_migrated(&pool, &migrator)
        .await
        .expect("begin migrated");
    guard
        .execute_bind(
            "INSERT INTO widgets (id, name) VALUES ($1, $2)",
            &[Value::Int(1), Value::Text("gadget".into())],
        )
        .await
        .expect("insert inside guard");
    guard.rollback().await.expect("rollback");

    // Second call is a no-op migration (schema already present) and the table
    // still exists because the migration ran outside the rolled-back tx.
    let applied = migrate_once(&pool, &migrator).await.expect("re-migrate");
    assert!(applied.is_empty(), "schema must be migrated once");
    assert_eq!(widget_count(&pool).await, 0, "no rows persisted");

    pool.close().await;
}

/// Negative: a panic (failed assertion) inside a guard still rolls back.
///
/// The panic unwinds inside a spawned task, dropping the guard without an
/// explicit `rollback()`; `sqlx`'s own `Transaction::Drop` queues the rollback
/// on the pooled connection, which runs on the next pool use.
#[tokio::test]
async fn assertion_failure_still_rolls_back() {
    let pool = memory_pool().await;
    let task_pool = pool.clone();

    let joined = tokio::spawn(async move {
        let mut guard = RefreshDatabase::begin(&task_pool).await.expect("begin");
        guard
            .execute_bind(
                "INSERT INTO widgets (id, name) VALUES ($1, $2)",
                &[Value::Int(9), Value::Text("doomed".into())],
            )
            .await
            .expect("insert inside guard");
        // Simulate a failed assertion: unwind without an explicit rollback.
        assert_eq!(1, 2, "intentional failure to exercise panic unwind");
        guard.rollback().await.expect("unreachable after panic");
    })
    .await;

    assert!(joined.is_err(), "task must have panicked");

    assert_eq!(
        widget_count(&pool).await,
        0,
        "panic path must still leave the table pristine"
    );
    pool.close().await;
}
