//! Queue schema migrations — `jobs` and `failed_jobs` tables.
//!
//! Dialect-portable DDL (TEXT columns, RFC3339 timestamps) so the same bodies
//! run on SQLite (tests) and Postgres. Register them with [`register`] at
//! application boot so `cargo artisan migrate` creates the queue tables.
//!
//! ## Table-name linkage to `[queue]`
//!
//! The default [`CreateJobsTable`] / [`CreateFailedJobsTable`] DDL hardcodes
//! `jobs`, `failed_jobs`, and `job_batches`. A custom
//! `[queue.connections.*.table]` / `[queue.failed].table` is honoured at the
//! driver level ([`crate::config::QueueConfig::jobs_table`] / `failed_table`
//! feed [`crate::driver::DatabaseDriver::with_tables`]); to also match the DDL,
//! build a migrator with [`migrator_with_tables`] (or register via
//! [`register_with_tables`]) and pass the configured names. The legacy
//! [`migrator`] / [`register`] keep the default table names so existing callers
//! are unaffected. `batching.table` is honoured by
//! [`crate::batch_db::DatabaseBatchRepository::with_table`]; use
//! [`register_with_tables_and_batches`] / [`migrator_with_batches_table`] to also
//! emit the `job_batches` DDL under a configured name.

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

/// `job_batches` table — persistent batch state and progress accounting.
///
/// Backs [`crate::batch_db::DatabaseBatchRepository`]: one row per dispatched
/// batch, carrying the aggregate counters (`total_jobs`/`pending_jobs`/
/// `failed_jobs`), the JSON `failed_job_ids` array, opaque `options`, and the
/// lifecycle timestamps (`created_at`/`finished_at`/`cancelled_at`).
pub struct CreateJobBatchesTable;

impl Migration for CreateJobBatchesTable {
    /// Unique migration name.
    fn name(&self) -> &str {
        "2027_01_01_000003_create_job_batches_table"
    }

    /// Create the `job_batches` table plus its lookup index.
    fn up(&self) -> Result<String> {
        Ok("\
CREATE TABLE IF NOT EXISTS job_batches (\
id TEXT PRIMARY KEY, \
name TEXT NOT NULL, \
total_jobs INTEGER NOT NULL DEFAULT 0, \
pending_jobs INTEGER NOT NULL DEFAULT 0, \
failed_jobs INTEGER NOT NULL DEFAULT 0, \
failed_job_ids TEXT NOT NULL DEFAULT '[]', \
options TEXT, \
created_at TEXT NOT NULL, \
finished_at TEXT, \
cancelled_at TEXT\
); \
CREATE INDEX IF NOT EXISTS idx_job_batches_pending ON job_batches (pending_jobs)"
            .to_string())
    }

    /// Drop the `job_batches` table.
    fn down(&self) -> Result<String> {
        Ok("DROP TABLE IF EXISTS job_batches".to_string())
    }
}

/// `queue_metrics` table — periodic queue-depth snapshots for the dashboard.
///
/// Backs [`crate::history::QueueMetricsHistory`]: one row per
/// `(sampled_at, connection, queue)` observation, so the dashboard can chart
/// pending/delayed/reserved depth and queue age over time. The composite primary
/// key makes a re-recorded identical sample idempotent, and the `sampled_at`
/// index keeps the retention prune and the `since` window scan cheap.
pub struct CreateQueueMetricsTable;

impl Migration for CreateQueueMetricsTable {
    /// Unique migration name.
    fn name(&self) -> &str {
        "2027_01_01_000004_create_queue_metrics_table"
    }

    /// Create the `queue_metrics` table plus its time index.
    fn up(&self) -> Result<String> {
        Ok("\
CREATE TABLE IF NOT EXISTS queue_metrics (\
sampled_at TEXT NOT NULL, \
connection TEXT NOT NULL, \
queue TEXT NOT NULL, \
pending INTEGER NOT NULL, \
delayed INTEGER NOT NULL, \
reserved INTEGER NOT NULL, \
oldest_pending TEXT, \
PRIMARY KEY (sampled_at, connection, queue)\
); \
CREATE INDEX IF NOT EXISTS idx_queue_metrics_sampled_at ON queue_metrics (sampled_at)"
            .to_string())
    }

    /// Drop the `queue_metrics` table.
    fn down(&self) -> Result<String> {
        Ok("DROP TABLE IF EXISTS queue_metrics".to_string())
    }
}

/// `job_batches` table migration with a configurable table name.
///
/// Mirrors [`CreateJobBatchesTable`] but emits DDL for `table` (from
/// `[queue.batching].table`). Used by [`migrator_with_batches_table`].
pub struct CreateJobBatchesTableNamed {
    /// Target table name.
    pub table: String,
}

impl Migration for CreateJobBatchesTableNamed {
    /// Unique migration name (fixed, so re-runs stay idempotent).
    fn name(&self) -> &str {
        "2027_01_01_000003_create_job_batches_table"
    }

    /// Create the named `job_batches` table plus its lookup index.
    fn up(&self) -> Result<String> {
        let table = &self.table;
        Ok(format!(
            "CREATE TABLE IF NOT EXISTS {table} (\
id TEXT PRIMARY KEY, \
name TEXT NOT NULL, \
total_jobs INTEGER NOT NULL DEFAULT 0, \
pending_jobs INTEGER NOT NULL DEFAULT 0, \
failed_jobs INTEGER NOT NULL DEFAULT 0, \
failed_job_ids TEXT NOT NULL DEFAULT '[]', \
options TEXT, \
created_at TEXT NOT NULL, \
finished_at TEXT, \
cancelled_at TEXT\
); \
CREATE INDEX IF NOT EXISTS idx_{table}_pending ON {table} (pending_jobs)"
        ))
    }

    /// Drop the named `job_batches` table.
    fn down(&self) -> Result<String> {
        Ok(format!("DROP TABLE IF EXISTS {}", self.table))
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
/// execution order: `jobs`, `failed_jobs`, then `job_batches`. Uses the default
/// table names; use [`register_with_tables_and_batches`] to honour a custom
/// `[queue]` config.
pub fn register() {
    REGISTERED.get_or_init(|| {
        rustasea_orm::register_migration(CreateJobsTable);
        rustasea_orm::register_migration(CreateFailedJobsTable);
        rustasea_orm::register_migration(CreateJobBatchesTable);
        rustasea_orm::register_migration(CreateQueueMetricsTable);
    });
}

/// Register the queue migrations using the configured table names.
///
/// Like [`register`] but emits DDL for `jobs_table` / `failed_table` (feed them
/// from [`crate::config::QueueConfig::jobs_table`] / `failed_table`). Shares the
/// same idempotence guard as [`register`], so calling both registers once.
pub fn register_with_tables(jobs_table: &str, failed_table: &str) {
    register_with_tables_and_batches(jobs_table, failed_table, crate::batch_db::JOB_BATCHES_TABLE);
}

/// Register the queue migrations using the configured table names.
///
/// Like [`register_with_tables`] but also emits the `job_batches` DDL for
/// `batches_table` (feed it from
/// [`crate::config::QueueConfig::batches_table`]). Shares the same idempotence
/// guard, so all registration entry points register exactly once.
pub fn register_with_tables_and_batches(jobs_table: &str, failed_table: &str, batches_table: &str) {
    REGISTERED.get_or_init(|| {
        rustasea_orm::register_migration(CreateJobsTableNamed {
            table: jobs_table.to_string(),
        });
        rustasea_orm::register_migration(CreateFailedJobsTableNamed {
            table: failed_table.to_string(),
        });
        rustasea_orm::register_migration(CreateJobBatchesTableNamed {
            table: batches_table.to_string(),
        });
        // The metrics table name is fixed (not configurable), so the default DDL
        // is registered alongside the named jobs/failed/batches variants.
        rustasea_orm::register_migration(CreateQueueMetricsTable);
    });
}

/// Build a migrator containing only the queue migrations (tests/tools).
///
/// Uses the default `jobs` / `failed_jobs` / `job_batches` table names; see
/// [`migrator_with_batches_table`] for the config-driven variant.
pub fn migrator() -> rustasea_orm::Migrator {
    let mut migrator = rustasea_orm::Migrator::new();
    migrator.add(CreateJobsTable);
    migrator.add(CreateFailedJobsTable);
    migrator.add(CreateJobBatchesTable);
    migrator.add(CreateQueueMetricsTable);
    migrator
}

/// Build a migrator with the configured `jobs` / `failed_jobs` table names.
///
/// Feed the names from [`crate::config::QueueConfig::jobs_table`] /
/// `failed_table` so the created schema matches what the `database` driver
/// queries. Uses the default `job_batches` table name; see
/// [`migrator_with_batches_table`] to also configure the batches table.
pub fn migrator_with_tables(jobs_table: &str, failed_table: &str) -> rustasea_orm::Migrator {
    migrator_with_batches_table(jobs_table, failed_table, crate::batch_db::JOB_BATCHES_TABLE)
}

/// Build a migrator with the configured `jobs` / `failed_jobs` / `job_batches`
/// table names.
///
/// Feed the names from [`crate::config::QueueConfig`] so the created schema
/// matches what the drivers and [`crate::batch_db::DatabaseBatchRepository`]
/// query.
pub fn migrator_with_batches_table(
    jobs_table: &str,
    failed_table: &str,
    batches_table: &str,
) -> rustasea_orm::Migrator {
    let mut migrator = rustasea_orm::Migrator::new();
    migrator.add(CreateJobsTableNamed {
        table: jobs_table.to_string(),
    });
    migrator.add(CreateFailedJobsTableNamed {
        table: failed_table.to_string(),
    });
    migrator.add(CreateJobBatchesTableNamed {
        table: batches_table.to_string(),
    });
    migrator.add(CreateQueueMetricsTable);
    migrator
}
