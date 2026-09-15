//! `FakeCache` — the `Cache::fake()` equivalent.
//!
//! [`FakeCache`] implements [`rustasea_cache::Store`], so it drops straight into
//! any seam that takes a store: construct one and hand it out as
//! `Arc<dyn Store>` / [`rustasea_cache::StoreRef`], or register it with
//! [`CacheManager::register`] via [`FakeCache::install`]. Every operation is
//! recorded and *also* forwarded to an internal [`rustasea_cache::MemoryStore`],
//! so `assert_has` / `assert_missing` observe real store state (TTL expiry,
//! forget, flush, compare-and-delete) rather than a stale op log. No Redis or
//! network is involved.
//!
//! Assertions come in two flavours: store-state checks ([`assert_has`] and
//! [`assert_missing`]) are `async` because they read the backing store, while
//! op-log checks ([`assert_put`], [`assert_forgotten`], [`assert_nothing_stored`])
//! are synchronous and inspect the recorded operations.
//!
//! [`assert_has`]: FakeCache::assert_has
//! [`assert_missing`]: FakeCache::assert_missing
//! [`assert_put`]: FakeCache::assert_put
//! [`assert_forgotten`]: FakeCache::assert_forgotten
//! [`assert_nothing_stored`]: FakeCache::assert_nothing_stored

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use rustasea_cache::{CacheManager, MemoryStore, Result, Store};

/// A single intercepted cache operation, in call order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedOp {
    /// The operation kind.
    pub op: Op,
    /// The key the operation targeted; empty for [`Op::Flush`].
    pub key: String,
}

/// Kind of intercepted cache operation.
///
/// [`Op::Put`], [`Op::PutIfAbsent`], [`Op::Touch`], and [`Op::Increment`] carry
/// the arguments the caller passed (value length, TTL, amount) so a test can
/// assert on them without reading the value back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// A `get`.
    Get,
    /// A `put`, with the stored value length and requested TTL.
    Put {
        /// Byte length of the stored value.
        len: usize,
        /// TTL the caller requested (zero means "forever").
        ttl: Duration,
    },
    /// A `put_if_absent`, with the stored value length and requested TTL.
    PutIfAbsent {
        /// Byte length of the stored value.
        len: usize,
        /// TTL the caller requested (zero means "forever").
        ttl: Duration,
    },
    /// A `compare_and_delete`.
    CompareAndDelete,
    /// A `touch`, with the requested TTL extension.
    Touch {
        /// TTL the caller requested for the extension.
        ttl: Duration,
    },
    /// A `forget`.
    Forget,
    /// A `flush`.
    Flush,
    /// An `increment` (also covers the default `decrement`, which routes here),
    /// with the signed amount added to the counter.
    Increment {
        /// Amount added to the counter (negative for a `decrement`).
        amount: i64,
    },
}

impl Op {
    /// Whether this operation mutates stored state.
    ///
    /// `put`, `put_if_absent`, and `increment`/`decrement` all change what the
    /// store holds, so write-detection assertions must treat them alike.
    fn is_write(self) -> bool {
        matches!(
            self,
            Op::Put { .. } | Op::PutIfAbsent { .. } | Op::Increment { .. }
        )
    }
}

impl RecordedOp {
    /// Human-readable one-line description used in failure summaries.
    fn describe(&self) -> String {
        match self.op {
            Op::Get => format!("get:{}", self.key),
            Op::Put { len, ttl } => format!("put:{} ({len} bytes, ttl {ttl:?})", self.key),
            Op::PutIfAbsent { len, ttl } => {
                format!("put_if_absent:{} ({len} bytes, ttl {ttl:?})", self.key)
            }
            Op::CompareAndDelete => format!("compare_and_delete:{}", self.key),
            Op::Touch { ttl } => format!("touch:{} (ttl {ttl:?})", self.key),
            Op::Forget => format!("forget:{}", self.key),
            Op::Flush => "flush".to_string(),
            Op::Increment { amount } => format!("increment:{} ({amount})", self.key),
        }
    }
}

/// Recording cache store — the `Cache::fake()` equivalent.
///
/// Implements [`Store`], delegating every call to an internal
/// [`MemoryStore`] (so reads reflect real semantics) while appending each
/// operation to a shared log. Hand it out as `Arc<dyn Store>` /
/// [`rustasea_cache::StoreRef`], or register it with a
/// [`CacheManager`] through [`install`](FakeCache::install).
///
/// ```no_run
/// use std::time::Duration;
/// use rustasea_cache::Store;
/// use rustasea_testing::FakeCache;
///
/// # async fn run() -> rustasea_cache::Result<()> {
/// let cache = FakeCache::new();
/// cache.put("user:1", b"ada".to_vec(), Duration::ZERO).await?;
/// cache.assert_has("user:1").await;
/// cache.assert_put("user:1");
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Default)]
pub struct FakeCache {
    /// Backing store — real TTL/atomic semantics, in memory.
    inner: Arc<MemoryStore>,
    /// Recorded operations, in call order.
    ops: Arc<Mutex<Vec<RecordedOp>>>,
}

impl FakeCache {
    /// Create an empty recording cache (not yet registered anywhere).
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a recording cache and register it under `name`.
    ///
    /// Returns the shared handle so the test can assert against the same
    /// instance the repository writes to. Callers that prefer direct
    /// construction can instead hand [`FakeCache`] out as
    /// [`rustasea_cache::StoreRef`].
    pub fn install(manager: &CacheManager, name: impl Into<String>) -> Arc<Self> {
        let fake = Arc::new(Self::default());
        manager.register(name, fake.clone());
        fake
    }

    /// Number of recorded operations (every `get`/`put`/… appends one).
    pub fn count(&self) -> usize {
        self.ops.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// Snapshot of every recorded operation, in call order.
    pub fn operations(&self) -> Vec<RecordedOp> {
        self.ops.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Keys touched by any write (`put`, `put_if_absent`, `increment`), sorted
    /// and de-duplicated.
    pub fn keys(&self) -> Vec<String> {
        let guard = self.ops.lock().unwrap_or_else(|p| p.into_inner());
        let mut keys: Vec<String> = guard
            .iter()
            .filter(|r| r.op.is_write())
            .map(|r| r.key.clone())
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// Assert `key` is currently present in the backing store.
    ///
    /// Reads the store, so TTL expiry, `forget`, `flush`, and a successful
    /// `compare_and_delete` all make this fail as expected.
    ///
    /// # Panics
    ///
    /// Panics with a message listing the recorded operations when `key` is
    /// absent.
    pub async fn assert_has(&self, key: &str) {
        let present = self.inner.get(key).await.ok().flatten().is_some();
        if !present {
            panic!(
                "expected key `{key}` to have been stored, but it was not.\nrecorded operations: [{}]",
                self.recorded_summary()
            );
        }
    }

    /// Assert `key` is currently absent from the backing store.
    ///
    /// # Panics
    ///
    /// Panics with a message listing the recorded operations when `key` is
    /// present.
    pub async fn assert_missing(&self, key: &str) {
        let present = self.inner.get(key).await.ok().flatten().is_some();
        if present {
            panic!(
                "expected key `{key}` to be missing, but it was stored.\nrecorded operations: [{}]",
                self.recorded_summary()
            );
        }
    }

    /// Assert a `put` (or `put_if_absent`) was recorded for `key`.
    ///
    /// Op-log based: it holds even if the key was later forgotten or expired.
    ///
    /// # Panics
    ///
    /// Panics with a message listing the recorded operations when no matching
    /// write was recorded.
    pub fn assert_put(&self, key: &str) {
        let recorded = self
            .ops
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .any(|r| r.key == key && matches!(r.op, Op::Put { .. } | Op::PutIfAbsent { .. }));
        if !recorded {
            panic!(
                "expected key `{key}` to have been put, but it was not.\nrecorded operations: [{}]",
                self.recorded_summary()
            );
        }
    }

    /// Assert a `forget` was recorded for `key`.
    ///
    /// # Panics
    ///
    /// Panics with a message listing the recorded operations when no `forget`
    /// was recorded.
    pub fn assert_forgotten(&self, key: &str) {
        let recorded = self
            .ops
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .any(|r| r.key == key && matches!(r.op, Op::Forget));
        if !recorded {
            panic!(
                "expected key `{key}` to have been forgotten, but it was not.\nrecorded operations: [{}]",
                self.recorded_summary()
            );
        }
    }

    /// Assert no write (`put`/`put_if_absent`/`increment`) was recorded at all.
    ///
    /// # Panics
    ///
    /// Panics listing the recorded operations when any write occurred.
    pub fn assert_nothing_stored(&self) {
        let writes = self
            .ops
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|r| r.op.is_write())
            .count();
        if writes != 0 {
            panic!(
                "expected nothing to have been stored, but {writes} write(s) were recorded.\nrecorded operations: [{}]",
                self.recorded_summary()
            );
        }
    }

    /// Drop every recorded operation and empty the backing store.
    pub async fn clear(&self) {
        // Flush the backing store directly (not via `Store::flush`) so the
        // reset itself does not append a spurious `Flush` to the fresh log.
        let _ = self.inner.flush().await;
        self.ops.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }

    /// Append an operation to the log (poison-tolerant).
    fn record(&self, op: Op, key: &str) {
        self.ops
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(RecordedOp {
                op,
                key: key.to_string(),
            });
    }

    /// Comma-separated list of recorded operations, for failures.
    fn recorded_summary(&self) -> String {
        let guard = self.ops.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .iter()
            .map(RecordedOp::describe)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[async_trait]
impl Store for FakeCache {
    /// Record the `get` and delegate to the backing store.
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.record(Op::Get, key);
        self.inner.get(key).await
    }

    /// Record the `put` (with value length + TTL) and delegate.
    async fn put(&self, key: &str, value: Vec<u8>, ttl: Duration) -> Result<()> {
        self.record(
            Op::Put {
                len: value.len(),
                ttl,
            },
            key,
        );
        self.inner.put(key, value, ttl).await
    }

    /// Record the `put_if_absent` (with value length + TTL) and delegate to the
    /// atomic store op.
    async fn put_if_absent(&self, key: &str, value: Vec<u8>, ttl: Duration) -> Result<bool> {
        self.record(
            Op::PutIfAbsent {
                len: value.len(),
                ttl,
            },
            key,
        );
        self.inner.put_if_absent(key, value, ttl).await
    }

    /// Record the `compare_and_delete` and delegate to the atomic store op.
    async fn compare_and_delete(&self, key: &str, expected: &[u8]) -> Result<bool> {
        self.record(Op::CompareAndDelete, key);
        self.inner.compare_and_delete(key, expected).await
    }

    /// Record the `touch` (with the requested TTL) and delegate to the store's
    /// real TTL extension.
    async fn touch(&self, key: &str, ttl: Duration) -> Result<bool> {
        self.record(Op::Touch { ttl }, key);
        self.inner.touch(key, ttl).await
    }

    /// Record the `forget` and delegate.
    async fn forget(&self, key: &str) -> Result<()> {
        self.record(Op::Forget, key);
        self.inner.forget(key).await
    }

    /// Record the `flush` and delegate.
    async fn flush(&self) -> Result<()> {
        self.record(Op::Flush, "");
        self.inner.flush().await
    }

    /// Record the `increment` (with the signed amount) and delegate.
    ///
    /// `decrement` uses the trait default, which routes through this method and
    /// is therefore recorded as [`Op::Increment`] with a negative amount too.
    async fn increment(&self, key: &str, n: i64) -> Result<i64> {
        self.record(Op::Increment { amount: n }, key);
        self.inner.increment(key, n).await
    }
}
