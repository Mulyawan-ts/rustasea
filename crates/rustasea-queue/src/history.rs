//! Queue metric history — snapshot persistence for the dashboard (ADOPT-021).
//!
//! [`QueueMetricsHistory`] persists periodic [`Queue::metrics`](crate::Queue::metrics)
//! snapshots into the `queue_metrics` table (see
//! [`crate::migrations::CreateQueueMetricsTable`]) so the dashboard can chart
//! queue depth and age over time. All SQL goes through the ORM runtime API
//! (`DbPool::fetch_json` / `execute_bind`) with `$n` positional binds, and
//! timestamps are stored as RFC3339 UTC micros strings — portable across
//! SQLite/Postgres/MySQL and comparable lexicographically.

use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use rustasea_orm::{DbPool, Value};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::metrics::Queues;

/// Name of the metric-history table created by
/// [`crate::migrations::CreateQueueMetricsTable`].
pub const QUEUE_METRICS_TABLE: &str = "queue_metrics";

/// One persisted queue-depth sample at a single `sampled_at` instant.
///
/// Mirrors [`crate::metrics::QueueMetrics`] but adds the sample timestamp so a
/// history window is a plain ordered list of these rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueMetricsSample {
    /// UTC instant this sample was taken.
    pub sampled_at: DateTime<Utc>,
    /// Connection (driver) name this row belongs to.
    pub connection: String,
    /// Queue name this row describes.
    pub queue: String,
    /// Number of available jobs at `sampled_at`.
    pub pending: usize,
    /// Number of delayed jobs at `sampled_at`.
    pub delayed: usize,
    /// Number of reserved in-flight jobs at `sampled_at`.
    pub reserved: usize,
    /// UTC instant of the oldest pending job; `None` on an empty queue.
    pub oldest_pending: Option<DateTime<Utc>>,
}

/// Persistence façade for queue-metric history (stateless namespace).
pub struct QueueMetricsHistory;

impl QueueMetricsHistory {
    /// Insert one row per `(connection, queue)` pair in `queues`.
    ///
    /// The rows share `sampled_at` and are written in a single multi-row
    /// `INSERT`, so a snapshot is atomic and cheap. Returns the number of rows
    /// written (zero for an empty snapshot). The composite primary key makes a
    /// re-recorded identical sample idempotent at the storage layer; this method
    /// itself does not de-duplicate.
    ///
    /// # Errors
    ///
    /// A typed [`crate::error::QueueError::StoreUnavailable`] when the insert
    /// fails (e.g. the `queue_metrics` table has not been migrated).
    pub async fn record_snapshot(
        pool: &DbPool,
        queues: &Queues,
        sampled_at: DateTime<Utc>,
    ) -> Result<usize> {
        if queues.queues.is_empty() {
            return Ok(0);
        }

        let sampled = format_timestamp(sampled_at);
        let mut sql = format!(
            "INSERT INTO {QUEUE_METRICS_TABLE} \
             (sampled_at, connection, queue, pending, delayed, reserved, oldest_pending) VALUES "
        );
        let mut binds: Vec<Value> = Vec::with_capacity(queues.queues.len() * 7);
        for (index, row) in queues.queues.iter().enumerate() {
            if index > 0 {
                sql.push_str(", ");
            }
            let base = index * 7;
            sql.push_str(&format!(
                "(${}, ${}, ${}, ${}, ${}, ${}, ${})",
                base + 1,
                base + 2,
                base + 3,
                base + 4,
                base + 5,
                base + 6,
                base + 7
            ));
            binds.push(Value::Text(sampled.clone()));
            binds.push(Value::Text(row.connection.clone()));
            binds.push(Value::Text(row.queue.clone()));
            binds.push(Value::Int(to_i64(row.pending)));
            binds.push(Value::Int(to_i64(row.delayed)));
            binds.push(Value::Int(to_i64(row.reserved)));
            binds.push(match row.oldest_pending {
                Some(instant) => Value::Text(format_timestamp(instant)),
                None => Value::Null,
            });
        }

        pool.execute_bind(&sql, &binds).await?;
        Ok(queues.queues.len())
    }

    /// Fetch every sample at or after `since`, oldest first.
    ///
    /// Ordering is `sampled_at`, then `connection`, then `queue`, so a caller
    /// can group consecutive rows into per-queue time series without re-sorting.
    ///
    /// # Errors
    ///
    /// A typed [`crate::error::QueueError::StoreUnavailable`] when the query
    /// fails.
    pub async fn fetch_history(
        pool: &DbPool,
        since: DateTime<Utc>,
    ) -> Result<Vec<QueueMetricsSample>> {
        let sql = format!(
            "SELECT sampled_at, connection, queue, pending, delayed, reserved, oldest_pending \
             FROM {QUEUE_METRICS_TABLE} WHERE sampled_at >= $1 \
             ORDER BY sampled_at ASC, connection ASC, queue ASC"
        );
        let rows = pool
            .fetch_json(&sql, &[Value::Text(format_timestamp(since))])
            .await?;
        Ok(rows.iter().filter_map(sample_from_row).collect())
    }

    /// Delete samples older than `retention` relative to the current instant.
    ///
    /// Returns the number of rows removed. Used by the sampler after every
    /// snapshot to keep the history table bounded by the configured window.
    ///
    /// # Errors
    ///
    /// A typed [`crate::error::QueueError::StoreUnavailable`] when the delete
    /// fails.
    pub async fn prune(pool: &DbPool, retention: Duration) -> Result<u64> {
        let cutoff = Utc::now() - chrono::Duration::from_std(retention).unwrap_or_default();
        let sql = format!("DELETE FROM {QUEUE_METRICS_TABLE} WHERE sampled_at < $1");
        let affected = pool
            .execute_bind(&sql, &[Value::Text(format_timestamp(cutoff))])
            .await?;
        Ok(affected)
    }
}

/// Format an instant as an RFC3339 UTC micros string (the storage convention).
fn format_timestamp(instant: DateTime<Utc>) -> String {
    instant.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Convert a `usize` count to the `i64` the ORM binds, saturating at the max.
fn to_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Decode one `queue_metrics` row, skipping malformed rows.
fn sample_from_row(row: &serde_json::Value) -> Option<QueueMetricsSample> {
    let sampled_at = row
        .get("sampled_at")
        .and_then(|v| v.as_str())
        .and_then(parse_timestamp)?;
    let oldest_pending = row
        .get("oldest_pending")
        .and_then(|v| v.as_str())
        .and_then(parse_timestamp);
    Some(QueueMetricsSample {
        sampled_at,
        connection: string_field(row, "connection"),
        queue: string_field(row, "queue"),
        pending: count_field(row, "pending"),
        delayed: count_field(row, "delayed"),
        reserved: count_field(row, "reserved"),
        oldest_pending,
    })
}

/// Parse an RFC3339 timestamp string into a UTC instant.
fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Read a string field from a JSON row, defaulting to an empty string.
fn string_field(row: &serde_json::Value, key: &str) -> String {
    row.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Read a non-negative count field, defaulting to zero.
fn count_field(row: &serde_json::Value, key: &str) -> usize {
    row.get(key)
        .and_then(|v| v.as_i64())
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::QueueMetrics;

    /// Build an in-memory pool with the metrics table created.
    async fn pool() -> DbPool {
        let pool = DbPool::connect("sqlite::memory:")
            .await
            .expect("sqlite pool");
        pool.execute_script(
            "CREATE TABLE queue_metrics (\
             sampled_at TEXT NOT NULL, connection TEXT NOT NULL, queue TEXT NOT NULL, \
             pending INTEGER NOT NULL, delayed INTEGER NOT NULL, reserved INTEGER NOT NULL, \
             oldest_pending TEXT, PRIMARY KEY (sampled_at, connection, queue)); \
             CREATE INDEX idx_queue_metrics_sampled_at ON queue_metrics (sampled_at)",
        )
        .await
        .expect("create table");
        pool
    }

    /// Build a snapshot with one row for `(connection, queue)`.
    fn snapshot(connection: &str, queue: &str, pending: usize) -> Queues {
        let mut queues = Queues::default();
        queues.upsert(QueueMetrics {
            connection: connection.to_string(),
            queue: queue.to_string(),
            pending,
            delayed: 1,
            reserved: 2,
            oldest_pending: None,
        });
        queues
    }

    /// `record_snapshot` writes one row per pair and returns the row count.
    #[tokio::test]
    async fn record_snapshot_inserts_one_row_per_pair() {
        let pool = pool().await;
        let mut queues = snapshot("database", "default", 3);
        queues.upsert(QueueMetrics {
            connection: "redis".to_string(),
            queue: "default".to_string(),
            pending: 5,
            delayed: 0,
            reserved: 0,
            oldest_pending: None,
        });
        let written = QueueMetricsHistory::record_snapshot(&pool, &queues, Utc::now())
            .await
            .expect("record");
        assert_eq!(written, 2);

        let history =
            QueueMetricsHistory::fetch_history(&pool, Utc::now() - chrono::Duration::days(1))
                .await
                .expect("fetch");
        assert_eq!(history.len(), 2);
    }

    /// `fetch_history` orders by timestamp then connection/queue and filters.
    #[tokio::test]
    async fn fetch_history_orders_and_filters_by_since() {
        let pool = pool().await;
        let early = Utc::now() - chrono::Duration::hours(2);
        let late = Utc::now() - chrono::Duration::minutes(5);
        QueueMetricsHistory::record_snapshot(&pool, &snapshot("database", "default", 1), early)
            .await
            .expect("early");
        QueueMetricsHistory::record_snapshot(&pool, &snapshot("database", "default", 9), late)
            .await
            .expect("late");

        let window =
            QueueMetricsHistory::fetch_history(&pool, Utc::now() - chrono::Duration::hours(1))
                .await
                .expect("fetch");
        assert_eq!(window.len(), 1, "only the recent sample is in the window");
        assert_eq!(window[0].pending, 9);

        let all = QueueMetricsHistory::fetch_history(&pool, Utc::now() - chrono::Duration::days(1))
            .await
            .expect("fetch all");
        assert_eq!(all.len(), 2);
        assert!(all[0].sampled_at <= all[1].sampled_at, "oldest first");
    }

    /// `prune` removes rows older than the retention window.
    #[tokio::test]
    async fn prune_removes_old_rows() {
        let pool = pool().await;
        let stale = Utc::now() - chrono::Duration::days(3);
        QueueMetricsHistory::record_snapshot(&pool, &snapshot("database", "default", 1), stale)
            .await
            .expect("stale");
        QueueMetricsHistory::record_snapshot(
            &pool,
            &snapshot("database", "default", 2),
            Utc::now(),
        )
        .await
        .expect("fresh");

        let removed = QueueMetricsHistory::prune(&pool, Duration::from_secs(24 * 3600))
            .await
            .expect("prune");
        assert_eq!(removed, 1, "only the stale row is pruned");

        let remaining =
            QueueMetricsHistory::fetch_history(&pool, Utc::now() - chrono::Duration::days(7))
                .await
                .expect("fetch");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].pending, 2);
    }

    /// An empty snapshot writes nothing.
    #[tokio::test]
    async fn record_snapshot_empty_is_noop() {
        let pool = pool().await;
        let written = QueueMetricsHistory::record_snapshot(&pool, &Queues::default(), Utc::now())
            .await
            .expect("record");
        assert_eq!(written, 0);
    }
}
