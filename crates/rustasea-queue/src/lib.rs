//! RustaSea Queue — typed jobs, central routing, drivers, chain/batch, metrics.
//!
//! Sprint 05 (M4) scope per sprint-05.md S05-T01..T03: the typed `Job` trait
//! with retry contracts, the `Queue::route::<Job>` registry with per-dispatch
//! `onQueue`/`onConnection` overrides, `sync`/`database`/`redis` drivers (the
//! `database` driver persists to the ORM `jobs` table; `redis` is opt-in behind
//! the `redis` feature), `chain`/`batch` dispatch, the `failed_jobs`
//! dead-letter surface and the Cloud queue metric shapes.

pub mod batch;
pub mod batch_db;
pub mod config;
pub mod dispatch;
pub mod driver;
pub mod error;
pub mod job;
pub mod metrics;
pub mod migrations;
pub mod notification;
pub mod policy;
pub mod registry;
pub mod retry;
pub mod unique;
pub mod wiring;

pub use async_trait::async_trait;
pub use batch::{dispatch_batch, BatchHandle, BatchId};
pub use batch_db::{
    batch_repository, clear_batch_repository, forget_batch_callbacks, on_catch, on_finally,
    on_then, record_batch_outcome, set_batch_repository, BatchCallback, BatchRecord,
    DatabaseBatchRepository, JOB_BATCHES_TABLE,
};
pub use config::{
    BatchingConfig, ConnectionConfig, FailedConfig, QueueConfig, DEFAULT_CONNECTION,
    DEFAULT_FAILED_TABLE, DEFAULT_JOBS_TABLE,
};
#[cfg(feature = "redis")]
pub use driver::RedisDriver;
pub use driver::{
    default_resolver, failed_jobs, register_job, register_job_handler, register_job_with_policy,
    retry_failed, run_worker, run_worker_with, DatabaseDriver, QueueDriver, SyncDriver,
    DATABASE_CONNECTION, DATABASE_DRIVER, REDIS_CONNECTION, REDIS_DRIVER, SYNC_CONNECTION,
};
pub use error::{JobError, QueueConfigError, QueueError, Result};
pub use job::{
    run_erased, ConcreteJob, DispatchHandle, ErasedJob, FailedJob, Job, JobId, JobOutcome,
    JobPayload,
};
pub use metrics::{JobQueueMetrics, QueueMetrics, Queues};
pub use migrations::{
    migrator as queue_migrator, migrator_with_batches_table, register as register_queue_migrations,
    register_with_tables_and_batches,
};
pub use notification::{
    should_suppress, skipped_notifications, NotificationGuard, NotificationSkipReason,
    NotificationSkipped,
};
pub use policy::JobPolicy;
#[cfg(feature = "redis")]
pub use registry::register_redis_driver;
pub use registry::{register_database_driver, Queue, QueueRegistry, Route};
pub use retry::{ShouldRetry, ShouldRetryUntil};
pub use unique::{
    acquire_lease, clear_unique_store, register_unique, release_unique, set_unique_store,
    unique_spec, unique_store, DispatchOutcome, LeaseState, ShouldBeUnique, UniqueGuard,
    DEFAULT_UNIQUE_FOR, UNIQUE_KEY_PREFIX,
};
pub use wiring::{configured_default_connection, register_from_config, register_from_config_with};
