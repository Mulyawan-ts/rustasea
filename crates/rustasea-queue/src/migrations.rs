//! Queue schema migrations — `jobs` and `failed_jobs` tables.
//!
//! Dialect-portable DDL (TEXT columns, RFC3339 timestamps) so the same bodies
//! run on SQLite (tests) and Postgres. Register them with [`register`] at
//! application boot so `cargo artisan migrate` creates the queue tables.
//!
//! ## Table-name linkage to `[queue]`
//!
//! The default [`CreateJobsTable`] / [`CreateFailedJobsTable`] DDL hardcodes
//! `jobs` and `failed_jobs`. A custom `[queue.connections.*.table]` /
//! `[queue.failed].table` is honoured at the driver level
//! ([`crate::config::QueueConfig::jobs_table`] / `failed_table` feed
//! [`crate::driver::DatabaseDriver::with_tables`]); to also match the DDL, build
//! a migrator with [`migrator_with_tables`] (or register via
//! [`register_with_tables`]) and pass the configured names. The legacy
//! [`migrator`] / [`register`] keep the default table names so existing callers
//! are unaffected. `batching.table` is exposed by
//! [`crate::config::QueueConfig::batches_table`] but has no migration yet.

use std::sync::OnceLock;

use rustasea_orm::{Migration, Result};

/// Guards [`register`] so the queue migrations enter the process-wide registry
/// exactly once even when both app boot and the CLI call it.
static REGISTERED: OnceLock<()> = OnceLock::new();

/// `jobs` table — pending, delayed, and reserved queue rows.
pub struct CreateJobsTable;

impl Migration for CreateJobsTable {
    /// Unique migration name.
    fn name(&self) -> &str {
        "2027_01_01_000001_create_jobs_table"
    }

    /// Create the `jobs` table plus its lookup indexes.
    fn up(&self) -> Result<String> {
        Ok("\
CREATE TABLE IF NOT EXISTS jobs (\
id TEXT PRIMARY KEY, \
queue TEXT NOT NULL, \
payload TEXT NOT NULL, \
attempts INTEGER NOT NULL DEFAULT 0, \
reserved_at TEXT, \
available_at TEXT NOT NULL, \
created_at TEXT NOT NULL\
); \
CREATE INDEX IF NOT EXISTS idx_jobs_queue_available ON jobs (queue, available_at); \
CREATE INDEX IF NOT EXISTS idx_jobs_reserved_at ON jobs (reserved_at)"
            .to_string())
    }

    /// Drop the `jobs` table.
    fn down(&self) -> Result<String> {
        Ok("DROP TABLE IF EXISTS jobs".to_string())
    }
}

/// `failed_jobs` table — dead-lettered jobs awaiting `queue:retry`.
pub struct CreateFailedJobsTable;

impl Migration for CreateFailedJobsTable {
    /// Unique migration name.
    fn name(&self) -> &str {
        "2027_01_01_000002_create_failed_jobs_table"
    }

    /// Create the `failed_jobs` table plus its lookup indexes.
    fn up(&self) -> Result<String> {
        Ok("\
CREATE TABLE IF NOT EXISTS failed_jobs (\
id TEXT PRIMARY KEY, \
connection TEXT NOT NULL, \
queue TEXT NOT NULL, \
payload TEXT NOT NULL, \
exception TEXT NOT NULL, \
failed_at TEXT NOT NULL\
); \
CREATE INDEX IF NOT EXISTS idx_failed_jobs_queue ON failed_jobs (queue); \
CREATE INDEX IF NOT EXISTS idx_failed_jobs_failed_at ON failed_jobs (failed_at)"
            .to_string())
    }

    /// Drop the `failed_jobs` table.
    fn down(&self) -> Result<String> {
        Ok("DROP TABLE IF EXISTS failed_jobs".to_string())
    }
}

/// `jobs` table migration with a configurable table name.
///
/// Mirrors [`CreateJobsTable`] but emits DDL for `table` (from
/// `[queue.connections.<name>].table`). Used by [`migrator_with_tables`].
pub struct CreateJobsTableNamed {
    /// Target table name.
    pub table: String,
}

impl Migration for CreateJobsTableNamed {
    /// Unique migration name (fixed, so re-runs stay idempotent).
    fn name(&self) -> &str {
        "2027_01_01_000001_create_jobs_table"
    }

    /// Create the named `jobs` table plus its lookup indexes.
    fn up(&self) -> Result<String> {
        let table = &self.table;
        Ok(format!(
            "CREATE TABLE IF NOT EXISTS {table} (\
id TEXT PRIMARY KEY, \
queue TEXT NOT NULL, \
payload TEXT NOT NULL, \
attempts INTEGER NOT NULL DEFAULT 0, \
reserved_at TEXT, \
available_at TEXT NOT NULL, \
created_at TEXT NOT NULL\
); \
CREATE INDEX IF NOT EXISTS idx_{table}_queue_available ON {table} (queue, available_at); \
CREATE INDEX IF NOT EXISTS idx_{table}_reserved_at ON {table} (reserved_at)"
        ))
    }

    /// Drop the named `jobs` table.
    fn down(&self) -> Result<String> {
        Ok(format!("DROP TABLE IF EXISTS {}", self.table))
    }
}

/// `failed_jobs` table migration with a configurable table name.
///
/// Mirrors [`CreateFailedJobsTable`] but emits DDL for `table` (from
/// `[queue.failed].table`). Used by [`migrator_with_tables`].
pub struct CreateFailedJobsTableNamed {
    /// Target table name.
    pub table: String,
}

impl Migration for CreateFailedJobsTableNamed {
    /// Unique migration name (fixed, so re-runs stay idempotent).
    fn name(&self) -> &str {
        "2027_01_01_000002_create_failed_jobs_table"
    }

    /// Create the named `failed_jobs` table plus its lookup indexes.
    fn up(&self) -> Result<String> {
        let table = &self.table;
        Ok(format!(
            "CREATE TABLE IF NOT EXISTS {table} (\
id TEXT PRIMARY KEY, \
connection TEXT NOT NULL, \
queue TEXT NOT NULL, \
payload TEXT NOT NULL, \
exception TEXT NOT NULL, \
failed_at TEXT NOT NULL\
); \
CREATE INDEX IF NOT EXISTS idx_{table}_queue ON {table} (queue); \
CREATE INDEX IF NOT EXISTS idx_{table}_failed_at ON {table} (failed_at)"
        ))
    }

    /// Drop the named `failed_jobs` table.
    fn down(&self) -> Result<String> {
        Ok(format!("DROP TABLE IF EXISTS {}", self.table))
    }
}

/// Register the queue migrations into the process-wide migrator.
///
/// Idempotent: repeated calls (app boot + CLI) register only once. Order is the
/// execution order: `jobs` then `failed_jobs`. Uses the default table names;
/// use [`register_with_tables`] to honour a custom `[queue]` config.
pub fn register() {
    REGISTERED.get_or_init(|| {
        rustasea_orm::register_migration(CreateJobsTable);
        rustasea_orm::register_migration(CreateFailedJobsTable);
    });
}

/// Register the queue migrations using the configured table names.
///
/// Like [`register`] but emits DDL for `jobs_table` / `failed_table` (feed them
/// from [`crate::config::QueueConfig::jobs_table`] / `failed_table`). Shares the
/// same idempotence guard as [`register`], so calling both registers once.
pub fn register_with_tables(jobs_table: &str, failed_table: &str) {
    REGISTERED.get_or_init(|| {
        rustasea_orm::register_migration(CreateJobsTableNamed {
            table: jobs_table.to_string(),
        });
        rustasea_orm::register_migration(CreateFailedJobsTableNamed {
            table: failed_table.to_string(),
        });
    });
}

/// Build a migrator containing only the queue migrations (tests/tools).
///
/// Uses the default `jobs` / `failed_jobs` table names; see
/// [`migrator_with_tables`] for the config-driven variant.
pub fn migrator() -> rustasea_orm::Migrator {
    let mut migrator = rustasea_orm::Migrator::new();
    migrator.add(CreateJobsTable);
    migrator.add(CreateFailedJobsTable);
    migrator
}

/// Build a migrator with the configured `jobs` / `failed_jobs` table names.
///
/// Feed the names from [`crate::config::QueueConfig::jobs_table`] /
/// `failed_table` so the created schema matches what the `database` driver
/// queries.
pub fn migrator_with_tables(jobs_table: &str, failed_table: &str) -> rustasea_orm::Migrator {
    let mut migrator = rustasea_orm::Migrator::new();
    migrator.add(CreateJobsTableNamed {
        table: jobs_table.to_string(),
    });
    migrator.add(CreateFailedJobsTableNamed {
        table: failed_table.to_string(),
    });
    migrator
}
