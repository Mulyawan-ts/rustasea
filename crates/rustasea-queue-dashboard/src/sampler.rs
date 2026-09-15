//! Background metrics sampler — periodic snapshot → record → prune loop.
//!
//! [`spawn_sampler`] spawns a tokio task that, every `interval`, takes a live
//! [`Queue::metrics`](rustasea_queue::Queue::metrics) snapshot, persists it with
//! [`QueueMetricsHistory::record_snapshot`](rustasea_queue::QueueMetricsHistory::record_snapshot),
//! and prunes samples older than `retention`. Errors are logged with `tracing`
//! and the loop continues — the sampler never panics and never aborts the
//! process, so a transient store error simply skips one sample.

use std::time::Duration;

use chrono::Utc;
use rustasea_orm::DbPool;
use rustasea_queue::{Queue, QueueMetricsHistory};
use tokio::task::JoinHandle;

/// Spawn the sampler loop, returning its [`JoinHandle`].
///
/// The first sample is taken after one `interval` (not immediately), so boot is
/// never blocked on the queue store. `retention` is the history window passed to
/// the prune step. Callers hold the handle (or let it run detached) — dropping
/// the runtime aborts it, which is the intended shutdown path for the scaffold.
///
/// # Panics
///
/// Never. Every failure inside the loop is logged and swallowed.
pub fn spawn_sampler(pool: DbPool, interval: Duration, retention: Duration) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Guard against a zero interval busy-looping the task.
        let period = if interval.is_zero() {
            Duration::from_secs(1)
        } else {
            interval
        };
        let mut ticker = tokio::time::interval(period);
        // Skip the immediate first tick so the loop starts on the first period.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            sample_once(&pool, retention).await;
        }
    })
}

/// Take one snapshot, persist it, then prune — logging and swallowing errors.
async fn sample_once(pool: &DbPool, retention: Duration) {
    let sampled_at = Utc::now();
    let queues = match Queue::metrics().await {
        Ok(queues) => queues,
        Err(error) => {
            tracing::warn!(%error, "queue dashboard sampler: metrics snapshot failed");
            return;
        }
    };
    match QueueMetricsHistory::record_snapshot(pool, &queues, sampled_at).await {
        Ok(rows) => {
            tracing::debug!(rows, "queue dashboard sampler: recorded snapshot");
        }
        Err(error) => {
            tracing::warn!(%error, "queue dashboard sampler: record_snapshot failed");
            return;
        }
    }
    if let Err(error) = QueueMetricsHistory::prune(pool, retention).await {
        tracing::warn!(%error, "queue dashboard sampler: prune failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustasea_queue::{QueueMetrics, Queues};

    /// Build an in-memory pool with the metrics table created.
    async fn pool() -> DbPool {
        let pool = DbPool::connect("sqlite::memory:").await.expect("pool");
        pool.execute_script(
            "CREATE TABLE queue_metrics (\
             sampled_at TEXT NOT NULL, connection TEXT NOT NULL, queue TEXT NOT NULL, \
             pending INTEGER NOT NULL, delayed INTEGER NOT NULL, reserved INTEGER NOT NULL, \
             oldest_pending TEXT, PRIMARY KEY (sampled_at, connection, queue))",
        )
        .await
        .expect("table");
        pool
    }

    /// `sample_once` records a snapshot when the queue registry has a route.
    ///
    /// The registry is process-wide and other tests may have added routes, so
    /// this asserts that *at least* the seeded route's row is persisted rather
    /// than an exact count.
    #[tokio::test]
    async fn sample_once_persists_snapshots() {
        use rustasea_queue::{Job, Queue, SyncDriver};
        use serde::{Deserialize, Serialize};
        use std::sync::Arc;

        #[derive(Debug, Clone, Serialize, Deserialize)]
        struct SamplerJob;

        #[rustasea_queue::async_trait]
        impl Job for SamplerJob {
            async fn handle(self) -> std::result::Result<(), rustasea_queue::JobError> {
                Ok(())
            }
        }

        let pool = pool().await;
        // Register the driver before the route so the metrics snapshot resolves.
        Queue::register_driver("sampler-conn", Arc::new(SyncDriver::new()));
        Queue::route::<SamplerJob>("sampler-conn", "sampler-queue").expect("route");

        sample_once(&pool, Duration::from_secs(3600)).await;

        let history =
            QueueMetricsHistory::fetch_history(&pool, Utc::now() - chrono::Duration::days(1))
                .await
                .expect("history");
        assert!(
            history
                .iter()
                .any(|s| s.queue == "sampler-queue" && s.connection == "sampler-conn"),
            "the seeded route must be persisted, got: {history:?}"
        );
    }

    /// `sample_once` is a no-op (no panic) when the store has no metrics table.
    #[tokio::test]
    async fn sample_once_survives_store_error() {
        let pool = DbPool::connect("sqlite::memory:").await.expect("pool");
        // No `queue_metrics` table: record_snapshot fails, the loop logs + skips.
        sample_once(&pool, Duration::from_secs(3600)).await;
    }

    /// A `Queues` snapshot round-trips through the history store.
    #[tokio::test]
    async fn snapshot_round_trips() {
        let pool = pool().await;
        let mut queues = Queues::default();
        queues.upsert(QueueMetrics {
            connection: "sampler-rt".to_string(),
            queue: "default".to_string(),
            pending: 4,
            delayed: 0,
            reserved: 0,
            oldest_pending: None,
        });
        QueueMetricsHistory::record_snapshot(&pool, &queues, Utc::now())
            .await
            .expect("record");
        let history =
            QueueMetricsHistory::fetch_history(&pool, Utc::now() - chrono::Duration::days(1))
                .await
                .expect("history");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].pending, 4);
    }
}
