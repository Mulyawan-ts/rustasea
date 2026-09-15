//! Process-wide install helpers — the causer/batch seams and a logger install.
//!
//! These thin wrappers forward to the ORM's audit registry so application code
//! can stay within the `rustasea-activitylog` namespace. The causer slot is the
//! request-scoped seam: auth middleware sets it, while CLI/queue contexts leave
//! it unset so the recorded causer is `None` (null-safe).

use std::sync::Arc;

use rustasea_orm::activity;

use crate::recorder::ActivityLogger;

/// Install `logger` as the process-wide activity recorder.
pub fn install(logger: Arc<ActivityLogger>) {
    activity::register_activity_recorder(logger);
}

/// Remove the process-wide activity recorder.
pub fn uninstall() {
    activity::clear_activity_recorder();
}

/// Install a causer resolver returning the current actor's id.
///
/// Auth middleware calls this with a closure reading the authenticated user;
/// CLI/queue contexts leave it unset so the causer stays `None`.
pub fn set_causer_resolver(resolver: activity::CauserResolver) {
    activity::set_causer_resolver(resolver);
}

/// Remove the causer resolver.
pub fn clear_causer_resolver() {
    activity::clear_causer_resolver();
}

/// The current causer id, or `None` when no resolver is installed.
pub fn current_causer() -> Option<String> {
    activity::current_causer()
}

/// Open a batch scope grouping related writes under `batch_uuid`.
///
/// The returned guard clears the batch id on drop.
pub fn batch_scope(batch_uuid: impl Into<String>) -> activity::BatchScope {
    activity::BatchScope::new(batch_uuid)
}

/// Set (or clear, with `None`) the batch id stamped on subsequent events.
pub fn set_batch_uuid(batch_uuid: Option<String>) {
    activity::set_batch_uuid(batch_uuid);
}

/// The current batch id, or `None` when no batch is open.
pub fn current_batch_uuid() -> Option<String> {
    activity::current_batch_uuid()
}
