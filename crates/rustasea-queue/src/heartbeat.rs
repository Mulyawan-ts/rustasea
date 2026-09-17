//! Worker heartbeats — process-wide last-seen stamps per queue (ADOPT-021).
//!
//! [`run_worker_with`](crate::driver::run_worker_with) stamps a heartbeat for
//! every queue it polls, so the dashboard can show which queues a worker has
//! touched recently and how long ago. The registry is a process-wide
//! `OnceLock<RwLock<HashMap<String, DateTime<Utc>>>>` keyed by queue name; a
//! heartbeat is a best-effort timestamp, not a lease, so a stale entry simply
//! means no worker has polled that queue recently.
//!
//! This is intentionally minimal: it is **not** a supervisor. It does not track
//! worker identity, concurrency, or liveness beyond "a worker polled this queue
//! at instant X", which is the only signal cheap enough to stamp inside the
//! worker loop without a new subsystem. Staleness is derived at read time via
//! [`is_active`] (see [`ACTIVE_WINDOW_SECS`]) rather than pruned, so a stopped
//! worker stays visible but muted instead of disappearing.
//!
//! The worker stamps on every pop cycle, as often as every `POLL_INTERVAL`
//! (25ms) when a queue is idle, so [`stamp`] takes the shared read lock first
//! and only upgrades to a write when the last persisted instant is older than
//! [`STAMP_REFRESH_SECS`]. The dashboard reads against the hour-scale
//! [`ACTIVE_WINDOW_SECS`] window, so a sub-second refresh cadence is
//! imperceptible while removing nearly all write-lock traffic from the hot loop.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use chrono::{DateTime, Utc};

/// Process-wide map from queue name to its most recent heartbeat instant.
static HEARTBEATS: OnceLock<RwLock<HashMap<String, DateTime<Utc>>>> = OnceLock::new();

/// How long after its most recent stamp a heartbeat is still considered active.
///
/// The worker loop stamps every queue it polls on each cycle, so a live worker
/// refreshes well inside this window; a stopped worker's entry ages past it and
/// is reported as inactive rather than silently vanishing, preserving the "this
/// queue was polled, but not recently" signal the dashboard renders as muted.
///
/// One hour is deliberately generous: it is far longer than the shipped 60s
/// sampler interval and any reasonable worker poll cadence, so a transient stall
/// never flaps the indicator, yet short enough that a genuinely stopped worker is
/// marked stale within a single dashboard session.
pub const ACTIVE_WINDOW_SECS: i64 = 3600;

/// Minimum gap, in seconds, between persisted heartbeat writes for one queue.
///
/// [`stamp`] skips the write lock when the queue's stored instant is younger
/// than this threshold. A live worker refreshes every second at most, so the
/// stored instant is always within one second of the most recent poll, far
/// tighter than the hour-scale [`ACTIVE_WINDOW_SECS`] the dashboard reads
/// against, making the coalescing behaviour-preserving for every consumer.
const STAMP_REFRESH_SECS: i64 = 1;

/// Whether `last_seen` falls within [`ACTIVE_WINDOW_SECS`] of `now`.
///
/// A `last_seen` in the future (clock skew) counts as active. Sub-second
/// truncation via [`chrono::Duration::num_seconds`] is immaterial at an
/// hour-scale window.
pub fn is_active(last_seen: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    now.signed_duration_since(last_seen).num_seconds() < ACTIVE_WINDOW_SECS
}

/// Access the heartbeat registry, initializing it on first use.
fn registry() -> &'static RwLock<HashMap<String, DateTime<Utc>>> {
    HEARTBEATS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Stamp `queue` with the current instant (called from the worker loop).
///
/// Best-effort: a poisoned lock is ignored so a heartbeat failure can never
/// abort a worker. Returns the instant observed.
///
/// The shared read lock is taken first; the exclusive write lock is only
/// acquired when the stored instant is stale (older than [`STAMP_REFRESH_SECS`]
/// or absent), so an idle worker polling in a tight loop does not serialize on
/// the registry. Concurrent readers never block one another.
pub fn stamp(queue: &str) -> DateTime<Utc> {
    let now = Utc::now();
    if let Ok(guard) = registry().read() {
        if let Some(last_seen) = guard.get(queue) {
            if now.signed_duration_since(*last_seen).num_seconds() < STAMP_REFRESH_SECS {
                return now;
            }
        }
    }
    stamp_at(queue, now);
    now
}

/// Stamp `queue` with an explicit `at` instant (used by tests).
pub fn stamp_at(queue: &str, at: DateTime<Utc>) {
    if let Ok(mut guard) = registry().write() {
        guard.insert(queue.to_string(), at);
    }
}

/// Snapshot every `(queue, last_seen)` heartbeat, ordered by queue name.
pub fn heartbeats() -> Vec<(String, DateTime<Utc>)> {
    let Some(registry) = HEARTBEATS.get() else {
        return Vec::new();
    };
    let Ok(guard) = registry.read() else {
        return Vec::new();
    };
    let mut entries: Vec<(String, DateTime<Utc>)> = guard
        .iter()
        .map(|(queue, at)| (queue.clone(), *at))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

/// Forget every heartbeat (test reset hook).
pub fn clear() {
    if let Some(registry) = HEARTBEATS.get() {
        if let Ok(mut guard) = registry.write() {
            guard.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Stamping a queue records and returns the instant, reads back, and a
    /// repeated `stamp` within [`STAMP_REFRESH_SECS`] is coalesced (no rewrite).
    #[test]
    fn stamp_records_reads_back_and_coalesces() {
        clear();
        let at = Utc
            .timestamp_opt(1_700_000_000, 0)
            .single()
            .expect("instant");
        stamp_at("heartbeat-test", at);
        let entries = heartbeats();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "heartbeat-test");
        assert_eq!(entries[0].1, at);

        // A fresh stored instant (well inside the threshold) is coalesced: the
        // repeat `stamp` returns a current instant but leaves the entry as-is.
        let fresh = Utc::now();
        stamp_at("coalesce-test", fresh);
        let second = stamp("coalesce-test");
        let entries = heartbeats();
        let coalesce = entries
            .iter()
            .find(|(queue, _)| queue == "coalesce-test")
            .expect("coalesce entry");
        assert_eq!(
            coalesce.1, fresh,
            "a fresh repeat stamp is coalesced, not rewritten"
        );
        assert!(
            second >= fresh,
            "the coalesced stamp still returns a current instant"
        );

        // A stale stored instant (older than the threshold) is refreshed.
        let stale = Utc::now() - chrono::Duration::seconds(STAMP_REFRESH_SECS + 1);
        stamp_at("coalesce-test", stale);
        let refreshed = stamp("coalesce-test");
        let entries = heartbeats();
        let coalesce = entries
            .iter()
            .find(|(queue, _)| queue == "coalesce-test")
            .expect("coalesce entry");
        assert!(
            coalesce.1 > stale,
            "a stale heartbeat is refreshed on the next stamp"
        );
        assert_eq!(coalesce.1, refreshed, "the refresh persists its instant");

        clear();
        assert!(heartbeats().is_empty());
    }

    /// A fresh stamp is active; one past the window is not; future skew is active.
    #[test]
    fn is_active_uses_the_window() {
        let now = Utc
            .timestamp_opt(1_700_000_000, 0)
            .single()
            .expect("instant");
        assert!(is_active(now, now), "a just-stamped heartbeat is active");
        assert!(
            is_active(now - chrono::Duration::seconds(ACTIVE_WINDOW_SECS - 1), now),
            "just inside the window is active"
        );
        assert!(
            !is_active(now - chrono::Duration::seconds(ACTIVE_WINDOW_SECS), now),
            "at the window edge is inactive"
        );
        assert!(
            !is_active(now - chrono::Duration::hours(2), now),
            "well past the window is inactive"
        );
        assert!(
            is_active(now + chrono::Duration::minutes(5), now),
            "a future (clock-skewed) stamp is active"
        );
    }
}
