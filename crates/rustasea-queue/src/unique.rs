//! `ShouldBeUnique` — deduplicate duplicate/concurrent dispatches with a cache lock.
//!
//! Laravel's `ShouldBeUnique` contract prevents the same logical job from being
//! enqueued twice: a unique identifier is used as an atomic cache lock, and a
//! second dispatch while the lock is held is silently dropped. This module
//! reproduces that behaviour on top of [`rustasea_cache::Lock`].
//!
//! # Reflection-free wiring
//!
//! Rust has no runtime reflection, so a generic `dispatch::<J>()` cannot ask
//! "does `J` implement `ShouldBeUnique`?". RustaSea already solves the identical
//! problem for `#[tries]`/`#[backoff]`/`#[timeout]` by *explicit registration*
//! (see [`crate::driver::register_job_with_policy`]); this module mirrors that
//! precedent: an application calls [`register_unique::<J>()`] once at boot for
//! each unique job type. Dispatch and worker paths then consult the registry by
//! the job's stable `type_key`.
//!
//! # Store wiring
//!
//! Unique locking requires a cache [`Store`]. It is **optional**: install one
//! with [`set_unique_store`] (or [`crate::Queue::set_unique_store`]) at boot.
//! When no store is set, unique locking *disengages* — dispatch behaves exactly
//! as before and [`LeaseState::NotApplicable`] is reported, preserving backward
//! compatibility for applications that never configure a store.
//!
//! # Lease lifecycle
//!
//! * On dispatch, [`acquire_lease`] inserts the key with `put_if_absent`
//!   (atomic `SET NX EX`); the lease persists under its TTL and is *not*
//!   released by the dispatching process.
//! * A worker that reaches a terminal outcome (`Succeeded`/`Skipped`/`Failed`)
//!   calls [`release_unique`] to delete the lease so the job may be dispatched
//!   again. A `Retrying` outcome keeps the lease (the job is still pending).
//! * If the holder crashes, the lease self-expires after `unique_for`.
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;

use rustasea_cache::{Lock, LockGuard, Store};

use crate::error::{QueueError, Result};
use crate::job::JobId;

/// Lease length applied when a job's `unique_for` returns zero.
///
/// A zero `unique_for` is treated as "use the sane default" rather than "never
/// expire", so a crashed holder can never wedge a unique job forever.
pub const DEFAULT_UNIQUE_FOR: Duration = Duration::from_secs(3600);

/// Key namespace for unique-lock entries in the cache store.
pub const UNIQUE_KEY_PREFIX: &str = "queue:unique:";

/// Opt-in contract marking a job as unique.
///
/// A job implementing this trait is enqueued at most once while its lease is
/// held. The default [`ShouldBeUnique::unique_id`] derives a stable identifier
/// from the job's fully-qualified type name plus a hash of its serialized
/// payload, so two structurally identical dispatches collide while two
/// dispatches carrying different payloads do not.
///
/// The default [`ShouldBeUnique::unique_for`] returns [`Duration::ZERO`], which
/// the acquisition path normalises to [`DEFAULT_UNIQUE_FOR`].
pub trait ShouldBeUnique {
    /// Stable unique identifier for this job instance.
    ///
    /// Defaults to `"{type_name}:{payload_hash}"`. Override to narrow the
    /// uniqueness scope (e.g. a domain id) so distinct payloads that should
    /// still deduplicate collide on the same id.
    fn unique_id(&self) -> String
    where
        Self: Serialize,
    {
        format!("{}:{}", std::any::type_name::<Self>(), payload_hash(self))
    }

    /// How long the uniqueness lease lasts.
    ///
    /// Defaults to [`Duration::ZERO`], normalised to [`DEFAULT_UNIQUE_FOR`] at
    /// acquisition. Override to bound how long a crashed dispatch can block a
    /// retry of the same logical job.
    fn unique_for(&self) -> Duration {
        Duration::ZERO
    }
}

/// Hash a serializable job body deterministically for the default unique id.
///
/// Uses [`std::collections::hash_map::DefaultHasher`], which is initialised
/// with fixed keys and is therefore stable across processes — dispatch and the
/// worker that later releases the lease must derive the same identifier. A
/// serialization failure falls back to hashing the type name.
fn payload_hash<J>(job: &J) -> u64
where
    J: Serialize + ?Sized,
{
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match serde_json::to_vec(job) {
        Ok(bytes) => bytes.hash(&mut hasher),
        Err(_) => std::any::type_name::<J>().hash(&mut hasher),
    }
    hasher.finish()
}

/// Normalise a job-declared TTL, mapping zero onto the default lease.
fn effective_ttl(ttl: Duration) -> Duration {
    if ttl.is_zero() {
        DEFAULT_UNIQUE_FOR
    } else {
        ttl
    }
}

/// Build the namespaced cache key for a unique id.
fn unique_key(id: &str) -> String {
    format!("{UNIQUE_KEY_PREFIX}{id}")
}

/// A factory deriving `(unique_id, unique_for)` from a serialized job body.
type UniqueFactory = Arc<dyn Fn(&serde_json::Value) -> Option<(String, Duration)> + Send + Sync>;

/// Process-wide map from job type name to its unique-spec factory.
static UNIQUE_REGISTRY: OnceLock<RwLock<HashMap<&'static str, UniqueFactory>>> = OnceLock::new();

/// Access the unique registry, initializing it on first use.
fn unique_registry() -> &'static RwLock<HashMap<&'static str, UniqueFactory>> {
    UNIQUE_REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Optional cache store backing unique locking.
static UNIQUE_STORE: OnceLock<RwLock<Option<Arc<dyn Store>>>> = OnceLock::new();

/// Access the store slot, initializing it on first use.
fn store_slot() -> &'static RwLock<Option<Arc<dyn Store>>> {
    UNIQUE_STORE.get_or_init(|| RwLock::new(None))
}

/// Install the cache store used for unique locking (boot-time).
///
/// Passing a store enables unique locking; leaving it unset (the default) makes
/// [`acquire_lease`] report [`LeaseState::NotApplicable`] so dispatch is
/// unchanged.
pub fn set_unique_store(store: Arc<dyn Store>) {
    let mut guard = store_slot().write().unwrap_or_else(|p| p.into_inner());
    *guard = Some(store);
}

/// Remove the configured unique-locking store (disengages unique locking).
pub fn clear_unique_store() {
    let mut guard = store_slot().write().unwrap_or_else(|p| p.into_inner());
    *guard = None;
}

/// The configured unique-locking store, or `None` when unset.
pub fn unique_store() -> Option<Arc<dyn Store>> {
    let guard = store_slot().read().unwrap_or_else(|p| p.into_inner());
    guard.clone()
}

/// Register `J` as a unique job type (boot-time).
///
/// Binds the job's [`ShouldBeUnique`] implementation into the process-wide
/// registry so the reflection-free dispatch/worker paths can derive its unique
/// spec from a serialized body. Registering the same type twice replaces the
/// prior binding. Mirrors [`crate::driver::register_job_with_policy`].
pub fn register_unique<J>()
where
    J: ShouldBeUnique + Serialize + DeserializeOwned + 'static,
{
    let key = std::any::type_name::<J>();
    let factory: UniqueFactory = Arc::new(|body: &serde_json::Value| {
        let job: J = serde_json::from_value(body.clone()).ok()?;
        Some((job.unique_id(), job.unique_for()))
    });
    let mut guard = unique_registry().write().unwrap_or_else(|p| p.into_inner());
    guard.insert(key, factory);
}

/// Derive the `(unique_id, unique_for)` spec for a job type + serialized body.
///
/// Returns `None` when the type was not registered via [`register_unique`], so
/// non-unique jobs pass through dispatch unchanged.
pub fn unique_spec(type_key: &str, body: &serde_json::Value) -> Option<(String, Duration)> {
    let registry = unique_registry();
    let guard = registry.read().unwrap_or_else(|p| p.into_inner());
    let factory = guard.get(type_key)?;
    factory(body)
}

/// Result of attempting to acquire a unique lease for one dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseState {
    /// The lease was acquired; the job may be enqueued.
    Acquired {
        /// Identifier the lease was taken under.
        unique_id: String,
    },
    /// The lease is already held; the dispatch must be deduplicated.
    Held {
        /// Identifier the lease is held under.
        unique_id: String,
    },
    /// No store is configured or the job type is not unique; dispatch as normal.
    NotApplicable,
}

/// Outcome of a dispatch that honours `ShouldBeUnique`.
///
/// Returned by [`crate::Queue::dispatch_handle_outcome`]. The historical
/// [`crate::Queue::dispatch_handle`] maps both variants onto `Ok(JobId)` for
/// backward compatibility (a deduplicated dispatch is a silent no-op, matching
/// Laravel), so callers that need to observe deduplication use the outcome form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// The job was enqueued; carries its new id.
    Enqueued(JobId),
    /// The job was skipped because an identical unique job is in flight.
    Deduplicated {
        /// Identifier the in-flight job holds.
        unique_id: String,
    },
}

/// Attempt to take the unique lease for `type_key`/`body` before enqueuing.
///
/// Returns [`LeaseState::NotApplicable`] when no store is configured or the
/// type is not registered as unique. Otherwise inserts the key atomically:
/// [`LeaseState::Acquired`] on success, [`LeaseState::Held`] when a lease is
/// already present. A store failure surfaces as `QueueError::StoreUnavailable`.
pub async fn acquire_lease(type_key: &str, body: &serde_json::Value) -> Result<LeaseState> {
    let Some(store) = unique_store() else {
        return Ok(LeaseState::NotApplicable);
    };
    let Some((unique_id, unique_for)) = unique_spec(type_key, body) else {
        return Ok(LeaseState::NotApplicable);
    };
    let key = unique_key(&unique_id);
    let token = format!("unique-{}", uuid::Uuid::new_v4()).into_bytes();
    match store
        .put_if_absent(&key, token, effective_ttl(unique_for))
        .await
    {
        Ok(true) => Ok(LeaseState::Acquired { unique_id }),
        Ok(false) => Ok(LeaseState::Held { unique_id }),
        Err(e) => Err(QueueError::StoreUnavailable(e.to_string())),
    }
}

/// Release the unique lease for `type_key`/`body` after a terminal outcome.
///
/// A no-op when no store is configured or the type is not unique. Uses an
/// unconditional delete (the worker did not acquire the lease, so it holds no
/// owner token); a lease that already expired is simply absent.
pub async fn release_unique(type_key: &str, body: &serde_json::Value) -> Result<()> {
    let Some(store) = unique_store() else {
        return Ok(());
    };
    let Some((unique_id, _)) = unique_spec(type_key, body) else {
        return Ok(());
    };
    store
        .forget(&unique_key(&unique_id))
        .await
        .map_err(|e| QueueError::StoreUnavailable(e.to_string()))
}

/// RAII helper wrapping a cache [`LockGuard`] for callers that want explicit
/// acquire/release semantics.
///
/// Unlike the dispatch path (which intentionally holds the lease under its TTL
/// so a *different* worker can release it), a `UniqueGuard` releases the lease
/// when it is dropped or when [`UniqueGuard::release`] is called — mirroring the
/// underlying [`LockGuard`] contract. Primarily useful for tests and for callers
/// that keep uniqueness scoped to a single process.
pub struct UniqueGuard {
    /// Underlying cache lock guard (releases on drop).
    inner: LockGuard,
    /// Identifier this guard holds.
    id: String,
}

impl UniqueGuard {
    /// Acquire the unique lease for `id`, or `None` when it is already held.
    ///
    /// `ttl` of zero is normalised to [`DEFAULT_UNIQUE_FOR`].
    pub async fn acquire(
        store: Arc<dyn Store>,
        id: impl Into<String>,
        ttl: Duration,
    ) -> Result<Option<Self>> {
        let id = id.into();
        let lock = Lock::new(store, unique_key(&id), effective_ttl(ttl));
        match lock.get().await {
            Ok(Some(inner)) => Ok(Some(Self { inner, id })),
            Ok(None) => Ok(None),
            Err(e) => Err(QueueError::StoreUnavailable(e.to_string())),
        }
    }

    /// Identifier this guard holds.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Explicitly release the lease (idempotent).
    pub async fn release(&mut self) -> Result<()> {
        self.inner
            .release()
            .await
            .map_err(|e| QueueError::StoreUnavailable(e.to_string()))
    }
}

impl std::fmt::Debug for UniqueGuard {
    /// Debug representation without the erased store.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UniqueGuard").field("id", &self.id).finish()
    }
}
