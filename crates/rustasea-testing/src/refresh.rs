//! `RefreshDatabase` — per-test transaction rollback (Laravel parity).
//!
//! Laravel's `Illuminate\Foundation\Testing\RefreshDatabase` trait wraps every
//! test in a database transaction and rolls it back afterwards, so a suite
//! reuses one migrated schema while each test still starts pristine. This module
//! is the rustasea equivalent for [`rustasea_orm`]: [`RefreshDatabase::begin`]
//! opens a [`Transaction`] on a live [`DbPool`] and returns a [`RefreshGuard`]
//! that the test body runs against.
//!
//! # Why this is fast
//!
//! [`crate::fixtures::PostgresTestDb`] starts a container per fixture (seconds).
//! `RefreshDatabase` starts *nothing* per test: provision the fixture and run
//! [`crate::migration::migrate_once`] once per binary, then wrap every test in a
//! guard so isolation comes from rollback alone.
//!
//! ```rust,ignore
//! use rustasea_testing::{migrate_once, PostgresTestDb, RefreshDatabase};
//!
//! let db = PostgresTestDb::start("suite").await?;
//! migrate_once(db.pool(), &migrator).await?;        // once per binary
//!
//! // ...per test:
//! let mut guard = RefreshDatabase::begin(db.pool()).await?;
//! guard
//!     .execute_bind("INSERT INTO widgets (id) VALUES ($1)", &[Value::Int(1)])
//!     .await?;
//! guard.rollback().await?;                           // table pristine again
//! ```
//!
//! # Rollback on drop
//!
//! Rust has no async drop, so a guard dropped without an explicit
//! [`RefreshGuard::rollback`] cannot *await* the rollback. Dropping the guard
//! still rolls back, because `sqlx`'s own `Transaction` `Drop` queues a rollback
//! that the driver runs on the connection's next use — including when the
//! connection is returned to the pool. Call [`RefreshGuard::rollback`] when the
//! test must observe the rollback before proceeding.

use rustasea_orm::{DbPool, Migrator, Result as OrmResult, Transaction, Value};

/// Laravel-style refresh helper: begin a per-test transaction, roll it back.
///
/// The type is a namespace of associated functions (mirroring
/// [`crate::fixtures::PostgresTestDb::start`]); the live handle it returns is a
/// [`RefreshGuard`].
pub struct RefreshDatabase;

impl RefreshDatabase {
    /// Begin a per-test transaction on `pool`.
    ///
    /// The returned [`RefreshGuard`] runs the test body on a single connection
    /// and rolls back on [`RefreshGuard::rollback`] (or, best-effort, on drop).
    /// Driver failures surface as [`rustasea_orm::OrmError`], which the harness
    /// lifts into [`crate::TestError`] via `From`.
    pub async fn begin(pool: &DbPool) -> OrmResult<RefreshGuard> {
        let transaction = Transaction::begin(pool).await?;
        Ok(RefreshGuard { transaction })
    }

    /// Apply pending migrations once, then begin a per-test transaction.
    ///
    /// Convenience for the common setup where the schema must exist exactly once
    /// per fixture but every test still starts pristine: [`crate::migration::migrate_once`]
    /// is a no-op after the first call for a given pool, so only the transaction
    /// is opened per test. The schema is created *outside* the transaction and
    /// therefore survives rollback; only per-test writes are discarded.
    pub async fn begin_migrated(pool: &DbPool, migrator: &Migrator) -> crate::Result<RefreshGuard> {
        crate::migration::migrate_once(pool, migrator).await?;
        Ok(Self::begin(pool).await?)
    }
}

/// An open per-test transaction, rolled back when the test finishes.
///
/// Obtain one from [`RefreshDatabase::begin`] (or
/// [`crate::fixtures::PostgresTestDb::begin_refresh`]) and run the test body
/// through [`RefreshGuard::transaction`]. Prefer an explicit
/// [`RefreshGuard::rollback`]; a guard dropped without one still rolls back via
/// the queued driver rollback (see the [module docs](self)).
pub struct RefreshGuard {
    /// The live transaction backing the test; single-use once closed.
    transaction: Transaction,
}

impl RefreshGuard {
    /// The transaction the test body should run against.
    ///
    /// Returns a mutable handle because every `rustasea_orm::Transaction`
    /// executor borrows `&mut self`. Do not [`commit`](Transaction::commit) it
    /// unless the test intentionally wants to persist writes.
    pub fn transaction(&mut self) -> &mut Transaction {
        &mut self.transaction
    }

    /// Alias of [`RefreshGuard::transaction`] under Laravel's `connection()` name.
    pub fn connection(&mut self) -> &mut Transaction {
        &mut self.transaction
    }

    /// Whether the underlying transaction is still open.
    pub fn is_open(&self) -> bool {
        self.transaction.is_open()
    }

    /// Run `sql` with `bindings` on the test transaction, returning rows affected.
    ///
    /// Convenience over [`RefreshGuard::transaction`]`().`[`Transaction::execute_bind`].
    pub async fn execute_bind(&mut self, sql: &str, bindings: &[Value]) -> OrmResult<u64> {
        self.transaction.execute_bind(sql, bindings).await
    }

    /// Execute a `;`-separated SQL script on the test transaction.
    ///
    /// Convenience over [`RefreshGuard::transaction`]`().`[`Transaction::execute_script`].
    pub async fn execute_script(&mut self, sql: &str) -> OrmResult<()> {
        self.transaction.execute_script(sql).await
    }

    /// Roll the transaction back, consuming the guard.
    ///
    /// This is the guaranteed path: it awaits the driver rollback before
    /// returning, so a caller that awaits it observes the pristine database.
    /// Returns [`rustasea_orm::OrmError`] if the driver rejects the rollback.
    pub async fn rollback(mut self) -> OrmResult<()> {
        self.transaction.rollback().await
    }

    /// Commit the transaction, consuming the guard (escape hatch).
    ///
    /// Intended only for tests that deliberately persist writes across the
    /// fixture; the default harness contract is rollback.
    pub async fn commit(mut self) -> OrmResult<()> {
        self.transaction.commit().await
    }
}

impl Drop for RefreshGuard {
    /// Best-effort rollback when the guard is dropped without an explicit call.
    ///
    /// Rust cannot await in `drop`, so this cannot call
    /// [`Transaction::rollback`]. Dropping the field instead lets `sqlx`'s own
    /// `Transaction::Drop` queue a rollback that runs on the connection's next
    /// async use (including return to the pool). Call [`RefreshGuard::rollback`]
    /// when the rollback must be observed deterministically.
    fn drop(&mut self) {
        // No-op body: `self.transaction` drops immediately after this returns,
        // and `sqlx`'s own `Transaction::Drop` queues the rollback that the
        // driver runs on the connection's next use. Kept explicit so the
        // drop-safety contract is greppable and unit-testable.
    }
}
