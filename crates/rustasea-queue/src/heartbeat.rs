//! Worker heartbeats — process-wide last-seen stamps per queue (ADOPT-021).
//!
//! [`run_worker_with`](crate::driver::run_worker_with) stamps a heartbeat for
//! every queue it polls, so the dashboard can show which queues a worker has
//! touched recently and how long ago. The registry is a process-wide
//! `OnceLock<Mutex<HashMap<String, DateTime<Utc>>>>` keyed by queue name; a
//! heartbeat is a best-effort timestamp, not a lease, so a stale entry simply
//! means no worker has polled that queue recently.
//!
//! This is intentionally minimal: it is **not** a supervisor. It does not track
//! worker identity, concurrency, or liveness beyond "a worker polled this queue
//! at instant X", which is the only signal cheap enough to stamp inside the
//! worker loop without a new subsystem. Staleness is derived at read time via
//! [`is_active`] (see [`ACTIVE_WINDOW_SECS`]) rather than pruned, so a stopped
//! worker stays visible but muted instead of disappearing.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Utc};

/// Process-wide map from queue name to its most recent heartbeat instant.
static HEARTBEATS: OnceLock<Mutex<HashMap<String, DateTime<Utc>>>> = OnceLock::new();

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

/// Whether `last_seen` falls within [`ACTIVE_WINDOW_SECS`] of `now`.
///
/// A `last_seen` in the future (clock skew) counts as active. Sub-second
/// truncation via [`chrono::Duration::num_seconds`] is immaterial at an
/// hour-scale window.
pub fn is_active(last_seen: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    now.signed_duration_since(last_seen).num_seconds() < ACTIVE_WINDOW_SECS
}

/// Access the heartbeat registry, initializing it on first use.
fn registry() -> &'static Mutex<HashMap<String, DateTime<Utc>>> {
    HEARTBEATS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Stamp `queue` with the current instant (called from the worker loop).
///
/// Best-effort: a poisoned lock is ignored so a heartbeat failure can never
/// abort a worker. Returns the instant recorded.
pub fn stamp(queue: &str) -> DateTime<Utc> {
    let now = Utc::now();
    stamp_at(queue, now);
    now
}

/// Stamp `queue` with an explicit `at` instant (used by tests).
pub fn stamp_at(queue: &str, at: DateTime<Utc>) {
    if let Ok(mut guard) = registry().lock() {
        guard.insert(queue.to_string(), at);
    }
}

/// Snapshot every `(queue, last_seen)` heartbeat, ordered by queue name.
pub fn heartbeats() -> Vec<(String, DateTime<Utc>)> {
    let Some(registry) = HEARTBEATS.get() else {
        return Vec::new();
    };
    let Ok(guard) = registry.lock() else {
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
        if let Ok(mut guard) = registry.lock() {
            guard.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Stamping a queue records and returns the instant, and is readable back.
    #[test]
    fn stamp_records_and_reads_back() {
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
