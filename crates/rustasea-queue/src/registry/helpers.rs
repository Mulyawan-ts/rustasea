//! Routing-registry internals shared by the [`crate::registry::Queue`] facade.
//!
//! Split out of `registry.rs` to keep that module within the file-size
//! standard. Holds the lock-free post-boot state ([`RegistryInner`]), route
//! registration, the serialized payload envelope builder, and driver
//! resolution — pure helpers with no `Queue` facade surface of their own. Every
//! item is re-exported from [`crate::registry`] so existing call paths are
//! unchanged.
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use crate::driver::{QueueDriver, SyncDriver, SYNC_CONNECTION};
use crate::error::{QueueError, Result};
use crate::job::JobPayload;

/// Lock-free post-boot state shared by every `Queue` facade.
static REGISTRY: OnceLock<RwLock<RegistryInner>> = OnceLock::new();

/// Interior of the central routing registry.
pub(crate) struct RegistryInner {
    /// Type-name -> resolved route for typed dispatch.
    pub(crate) routes: HashMap<&'static str, Route>,
    /// Named drivers available for dispatch.
    pub(crate) drivers: HashMap<String, Arc<dyn QueueDriver>>,
}

/// A resolved route: connection + queue for one job type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// Connection (driver) name.
    pub connection: &'static str,
    /// Target queue name.
    pub queue: &'static str,
}

impl Route {
    /// Queue this route targets.
    pub fn queue_name(&self) -> &'static str {
        self.queue
    }

    /// Connection this route targets.
    pub fn connection_name(&self) -> &'static str {
        self.connection
    }
}

/// Returns the registry inner, initializing it with a sync driver on first use.
pub(crate) fn registry() -> &'static RwLock<RegistryInner> {
    REGISTRY.get_or_init(|| {
        let mut inner = RegistryInner {
            routes: HashMap::new(),
            drivers: HashMap::new(),
        };
        inner
            .drivers
            .insert(SYNC_CONNECTION.to_string(), Arc::new(SyncDriver::new()));
        RwLock::new(inner)
    })
}

/// Registers a route for `type_key` under `connection`/`queue`.
pub(crate) fn insert_route(type_key: &'static str, route: Route) -> Result<()> {
    let reg = registry();
    let mut guard = reg.write().map_err(QueueError::from)?;
    if guard.routes.contains_key(type_key) {
        return Err(QueueError::DuplicateRoute {
            type_name: type_key,
        });
    }
    guard.routes.insert(type_key, route);
    Ok(())
}

/// Build the serialized payload envelope for a dispatch.
pub(crate) fn to_payload(
    job: &str,
    payload: serde_json::Value,
    queue: String,
    connection: String,
    delay: Duration,
    batch_id: Option<String>,
) -> Result<JobPayload> {
    let available_at = if delay.is_zero() {
        None
    } else {
        Some(chrono::Utc::now() + chrono::Duration::from_std(delay).unwrap_or_default())
    };
    Ok(JobPayload {
        queue,
        connection,
        available_at,
        attempts: 1,
        id: None,
        job: Some(job.to_string()),
        batch_id,
        payload,
    })
}

/// Unique `(connection, queue)` pairs registered through `Queue::route`.
///
/// Many job types may route to the same queue; deduplicating yields one metric
/// row per configured queue. The routes are snapshotted under the lock and
/// returned owned so `Queue::metrics` never holds the registry lock across an
/// `await`.
pub(crate) fn metric_targets() -> Vec<(String, String)> {
    let reg = registry();
    let Ok(guard) = reg.read() else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut targets = Vec::new();
    for route in guard.routes.values() {
        let key = (route.connection.to_string(), route.queue.to_string());
        if seen.insert(key.clone()) {
            targets.push(key);
        }
    }
    targets
}

/// Resolve a driver by connection name.
pub(crate) fn driver(connection: &str) -> Result<Arc<dyn QueueDriver>> {
    let reg = registry();
    let guard = reg.read().map_err(QueueError::from)?;
    guard
        .drivers
        .get(connection)
        .cloned()
        .ok_or_else(|| QueueError::UnknownConnection(connection.to_string()))
}
