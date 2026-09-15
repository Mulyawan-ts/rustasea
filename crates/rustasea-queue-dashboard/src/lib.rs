//! RustaSea Queue Dashboard — metrics history + failed-job management (ADOPT-021).
//!
//! Parity target: `laravel/horizon`. The crate assembles the data a queue
//! dashboard needs and owns the background sampler that persists it:
//!
//! * [`DashboardConfig`] parses the `[queue.dashboard]` table (`enabled`,
//!   `retention_minutes`, `sample_interval_secs`).
//! * [`DashboardData`] is the stateless read/write façade: the live
//!   [`Queue::metrics`](rustasea_queue::Queue::metrics) snapshot, the persisted
//!   history ([`QueueMetricsHistory`](rustasea_queue::QueueMetricsHistory)), the
//!   failed-job list, and the `retry`/`forget`/`prune` actions.
//! * [`sampler::spawn_sampler`] runs the periodic snapshot→record→prune loop.
//!
//! # Pool ownership
//!
//! The crate never opens a connection itself: every data method takes a
//! caller-supplied [`DbPool`]. The runnable app resolves the database URL at
//! boot, connects once, installs the pool with [`set_pool`], and both the
//! sampler and the route handlers reuse it — so a request never opens its own
//! connection.
//!
//! # Production safety
//!
//! Nothing runs unless the app enables the feature and sets
//! `[queue.dashboard].enabled = true`; the route surface additionally requires
//! `AppState::debug`, so a production process exposes neither the UI nor the
//! sampler.

pub mod sampler;

use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rustasea_config::ConfigLoader;
use rustasea_queue::{heartbeat, DatabaseDriver, Queue, QueueMetricsHistory};
use serde::{Deserialize, Serialize};

/// The ORM pool the dashboard reads/writes through (re-exported so consumers
/// need not depend on `rustasea-orm` directly).
pub use rustasea_orm::DbPool;
/// The `queue_metrics` history table name (re-exported for the migration docs).
pub use rustasea_queue::QUEUE_METRICS_TABLE;
/// Queue types the dashboard surface exposes (re-exported so consumers need not
/// depend on `rustasea-queue` directly).
pub use rustasea_queue::{FailedJob, JobId, QueueMetricsSample, Queues};

/// Typed configuration error for the `[queue.dashboard]` table.
#[derive(Debug, thiserror::Error)]
pub enum DashboardError {
    /// The table exists but could not be deserialized.
    #[error("invalid [queue.dashboard] config: {0}")]
    InvalidConfig(String),
}

/// Result alias for dashboard configuration.
pub type Result<T> = std::result::Result<T, DashboardError>;

/// Default history retention window (24 hours).
pub const DEFAULT_RETENTION_MINUTES: u64 = 1440;
/// Default sampler interval (60 seconds).
pub const DEFAULT_SAMPLE_INTERVAL_SECS: u64 = 60;

/// Parsed `[queue.dashboard]` configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardConfig {
    /// Whether the dashboard surface and sampler are enabled.
    pub enabled: bool,
    /// History retention window in minutes.
    pub retention_minutes: u64,
    /// Sampler interval in seconds.
    pub sample_interval_secs: u64,
}

impl Default for DashboardConfig {
    /// A disabled dashboard with the shipped defaults.
    fn default() -> Self {
        Self {
            enabled: false,
            retention_minutes: DEFAULT_RETENTION_MINUTES,
            sample_interval_secs: DEFAULT_SAMPLE_INTERVAL_SECS,
        }
    }
}

/// Deserialized shape of the `[queue.dashboard]` table.
#[derive(Debug, Default, Deserialize)]
struct DashboardTable {
    /// Enable flag from the config file.
    #[serde(default)]
    enabled: Option<bool>,
    /// Retention minutes from the config file.
    #[serde(default)]
    retention_minutes: Option<u64>,
    /// Sampler interval seconds from the config file.
    #[serde(default)]
    sample_interval_secs: Option<u64>,
}

impl DashboardConfig {
    /// Load `[queue.dashboard]` from `loader`.
    ///
    /// A missing table is tolerated (a disabled config); a malformed one
    /// surfaces [`DashboardError::InvalidConfig`]. Zero values fall back to the
    /// shipped defaults so a stray `0` never disables retention or busy-loops
    /// the sampler.
    ///
    /// # Errors
    ///
    /// [`DashboardError::InvalidConfig`] when `[queue.dashboard]` exists but
    /// cannot be deserialized.
    pub fn from_loader(loader: &ConfigLoader) -> Result<Self> {
        let table = match loader.get_key::<DashboardTable>("queue.dashboard") {
            Ok(table) => table,
            Err(error) => {
                if loader.inner().get_table("queue.dashboard").is_err() {
                    DashboardTable::default()
                } else {
                    return Err(DashboardError::InvalidConfig(error.to_string()));
                }
            }
        };
        Ok(Self {
            enabled: table.enabled.unwrap_or(false),
            retention_minutes: non_zero(table.retention_minutes, DEFAULT_RETENTION_MINUTES),
            sample_interval_secs: non_zero(
                table.sample_interval_secs,
                DEFAULT_SAMPLE_INTERVAL_SECS,
            ),
        })
    }

    /// The history retention window as a [`Duration`].
    pub fn retention(&self) -> Duration {
        Duration::from_secs(self.retention_minutes.saturating_mul(60))
    }

    /// The sampler interval as a [`Duration`].
    pub fn sample_interval(&self) -> Duration {
        Duration::from_secs(self.sample_interval_secs)
    }
}

/// Fall back to `default` when `value` is `None` or zero.
fn non_zero(value: Option<u64>, default: u64) -> u64 {
    match value {
        Some(v) if v > 0 => v,
        _ => default,
    }
}

/// Process-wide parsed config installed by the app at boot.
static CONFIG: OnceLock<RwLock<DashboardConfig>> = OnceLock::new();

/// Access the config cell, initializing it to the disabled default.
fn config_cell() -> &'static RwLock<DashboardConfig> {
    CONFIG.get_or_init(|| RwLock::new(DashboardConfig::default()))
}

/// Install the parsed config (called by the app at boot).
///
/// Replaces any previously installed config, so a test can enable the surface
/// and a later `configure()` can restore the configured value.
pub fn set_config(config: DashboardConfig) {
    if let Ok(mut guard) = config_cell().write() {
        *guard = config;
    }
}

/// The installed config, or the disabled default before [`set_config`] runs.
pub fn config() -> DashboardConfig {
    config_cell()
        .read()
        .map(|guard| guard.clone())
        .unwrap_or_default()
}

/// Whether the dashboard surface is enabled (config flag).
pub fn enabled() -> bool {
    config().enabled
}

/// Process-wide pool installed by the app at boot.
///
/// Held in a [`RwLock`] (not a `OnceLock`) so the route tests can install a
/// throwaway in-memory pool and reset it afterwards; production installs it once
/// at boot and never replaces it.
static POOL: OnceLock<RwLock<Option<DbPool>>> = OnceLock::new();

/// Access the pool cell, initializing it to empty on first use.
fn pool_cell() -> &'static RwLock<Option<DbPool>> {
    POOL.get_or_init(|| RwLock::new(None))
}

/// Install the shared pool, replacing any previously installed pool.
///
/// The app calls this once at boot; the sampler and every route handler read it
/// back through [`pool`], so the process opens exactly one pool. Returns the
/// previous pool when one was already installed.
pub fn set_pool(pool: DbPool) -> Option<DbPool> {
    match pool_cell().write() {
        Ok(mut guard) => guard.replace(pool),
        Err(_) => None,
    }
}

/// Clear the shared pool (test reset hook).
pub fn clear_pool() {
    if let Ok(mut guard) = pool_cell().write() {
        *guard = None;
    }
}

/// The installed shared pool, or `None` before [`set_pool`] runs.
pub fn pool() -> Option<DbPool> {
    pool_cell().read().ok().and_then(|guard| guard.clone())
}

/// One live worker heartbeat (queue name + last-seen instant).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerHeartbeat {
    /// Queue the worker last polled.
    pub queue: String,
    /// UTC instant of the most recent poll.
    pub last_seen: DateTime<Utc>,
    /// Whether the heartbeat is recent enough to count as a live worker.
    ///
    /// Derived at read time from [`heartbeat::is_active`] against the
    /// [`heartbeat::ACTIVE_WINDOW_SECS`] window (one hour): a stopped worker
    /// stays in the list — so an operator still sees *which* queue went quiet —
    /// but is flagged `false` and rendered muted by the dashboard. Annotating
    /// rather than filtering keeps the JSON shape additive and lets a client
    /// choose its own display policy.
    pub active: bool,
}

/// The JSON payload served by the metrics endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsResponse {
    /// Live queue-depth snapshot from [`Queue::metrics`].
    pub snapshot: Queues,
    /// Persisted history samples inside the requested window.
    pub history: Vec<QueueMetricsSample>,
    /// Live worker heartbeats (empty when no worker has polled yet).
    pub workers: Vec<WorkerHeartbeat>,
}

/// Stateless read/write façade over the queue dashboard data.
pub struct DashboardData;

impl DashboardData {
    /// Live queue-depth snapshot across every routed `(connection, queue)`.
    ///
    /// Delegates to [`Queue::metrics`], so the registry must already have the
    /// connection drivers registered (the app does this at boot).
    ///
    /// # Errors
    ///
    /// A typed [`rustasea_queue::QueueError`] from the registry or a driver.
    pub async fn snapshot() -> rustasea_queue::Result<Queues> {
        Queue::metrics().await
    }

    /// Persisted history samples at or after `since`, oldest first.
    ///
    /// # Errors
    ///
    /// A typed [`rustasea_queue::QueueError`] when the query fails.
    pub async fn history(
        pool: &DbPool,
        since: DateTime<Utc>,
    ) -> rustasea_queue::Result<Vec<QueueMetricsSample>> {
        QueueMetricsHistory::fetch_history(pool, since).await
    }

    /// List persisted dead-letter rows, oldest first.
    ///
    /// # Errors
    ///
    /// A typed [`rustasea_queue::QueueError`] when the query fails.
    pub async fn failed_jobs(pool: &DbPool) -> rustasea_queue::Result<Vec<FailedJob>> {
        DatabaseDriver::new(pool.clone()).failed_jobs().await
    }

    /// Re-enqueue a dead-lettered job, deleting the row only after a push.
    ///
    /// # Errors
    ///
    /// A typed [`rustasea_queue::QueueError`] when the row is missing or the
    /// re-push fails.
    pub async fn retry_failed(pool: &DbPool, id: JobId) -> rustasea_queue::Result<()> {
        DatabaseDriver::new(pool.clone()).retry_failed(id).await
    }

    /// Permanently forget a dead-lettered job, returning whether a row matched.
    ///
    /// # Errors
    ///
    /// A typed [`rustasea_queue::QueueError`] when the delete fails.
    pub async fn forget_failed(pool: &DbPool, id: JobId) -> rustasea_queue::Result<bool> {
        DatabaseDriver::new(pool.clone()).forget_failed(id).await
    }

    /// Prune history samples older than `retention`, returning rows removed.
    ///
    /// # Errors
    ///
    /// A typed [`rustasea_queue::QueueError`] when the delete fails.
    pub async fn prune(pool: &DbPool, retention: Duration) -> rustasea_queue::Result<u64> {
        QueueMetricsHistory::prune(pool, retention).await
    }

    /// Snapshot the live worker heartbeats, ordered by queue name.
    ///
    /// Each entry is annotated with `active` (see [`WorkerHeartbeat::active`]):
    /// a heartbeat older than [`heartbeat::ACTIVE_WINDOW_SECS`] is reported but
    /// flagged inactive, so a stopped worker stays visible as muted rather than
    /// silently dropping off the dashboard.
    pub fn workers() -> Vec<WorkerHeartbeat> {
        let now = Utc::now();
        heartbeat::heartbeats()
            .into_iter()
            .map(|(queue, last_seen)| WorkerHeartbeat {
                queue,
                active: heartbeat::is_active(last_seen, now),
                last_seen,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// A missing `[queue.dashboard]` table yields the disabled default.
    #[test]
    fn missing_table_defaults_to_disabled() {
        let dir = std::env::temp_dir().join(format!("qd-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("app.toml"), "[app]\nname = \"x\"\n").expect("write");
        let loader = ConfigLoader::load_from_dir(&dir).expect("loader");
        let config = DashboardConfig::from_loader(&loader).expect("config");
        assert_eq!(config, DashboardConfig::default());
        assert!(!config.enabled);
        assert_eq!(config.retention_minutes, DEFAULT_RETENTION_MINUTES);
        assert_eq!(config.sample_interval_secs, DEFAULT_SAMPLE_INTERVAL_SECS);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The `[queue.dashboard]` values are read, with zero falling back.
    #[test]
    fn reads_values_and_rejects_zero() {
        let dir = std::env::temp_dir().join(format!("qd-cfg2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(
            dir.join("queue.toml"),
            "[queue.dashboard]\nenabled = true\nretention_minutes = 120\nsample_interval_secs = 0\n",
        )
        .expect("write");
        let loader = ConfigLoader::load_from_dir(&dir).expect("loader");
        let config = DashboardConfig::from_loader(&loader).expect("config");
        assert!(config.enabled);
        assert_eq!(config.retention_minutes, 120);
        assert_eq!(
            config.sample_interval_secs, DEFAULT_SAMPLE_INTERVAL_SECS,
            "zero falls back to the default"
        );
        assert_eq!(config.retention(), Duration::from_secs(7200));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A malformed `[queue.dashboard]` table is a typed error.
    #[test]
    fn malformed_table_is_typed_error() {
        let dir = std::env::temp_dir().join(format!("qd-cfg3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(
            dir.join("queue.toml"),
            "[queue.dashboard]\nenabled = \"not-a-bool\"\n",
        )
        .expect("write");
        let loader = ConfigLoader::load_from_dir(&dir).expect("loader");
        let error = DashboardConfig::from_loader(&loader).expect_err("malformed");
        assert!(matches!(error, DashboardError::InvalidConfig(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `workers` renders the heartbeat registry as ordered, active rows.
    #[test]
    fn workers_renders_heartbeats() {
        heartbeat::clear();
        let at = Utc
            .timestamp_opt(1_700_000_000, 0)
            .single()
            .expect("instant");
        heartbeat::stamp_at("qd-workers", at);
        let workers = DashboardData::workers();
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].queue, "qd-workers");
        assert_eq!(workers[0].last_seen, at);
        heartbeat::clear();
    }

    /// A stale heartbeat is reported but flagged `active == false`.
    #[test]
    fn workers_flags_stale_heartbeats_inactive() {
        heartbeat::clear();
        let now = Utc::now();
        heartbeat::stamp_at("qd-fresh", now);
        heartbeat::stamp_at(
            "qd-stale",
            now - chrono::Duration::seconds(heartbeat::ACTIVE_WINDOW_SECS + 1),
        );
        let workers = DashboardData::workers();
        let fresh = workers
            .iter()
            .find(|w| w.queue == "qd-fresh")
            .expect("fresh entry");
        let stale = workers
            .iter()
            .find(|w| w.queue == "qd-stale")
            .expect("stale entry is retained, not filtered");
        assert!(fresh.active, "a just-stamped heartbeat is active");
        assert!(!stale.active, "a heartbeat past the window is inactive");
        heartbeat::clear();
    }
}
