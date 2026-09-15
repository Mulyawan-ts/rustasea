//! RustaSea activity log — model-change audit trail (spatie/laravel-activitylog parity).
//!
//! The audit trail records who changed what, when, with a JSON property diff,
//! grouped by an optional `batch_uuid`. It builds on the ORM's audit seam
//! ([`rustasea_orm::activity`]): a model opts in with `#[logs_activity]`, the
//! ORM write path builds an [`ActivityEvent`] for each create/update/delete,
//! and an installed [`ActivityLogger`] persists it to the `audit_log` table.
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use rustasea_activitylog::ActivityLogger;
//!
//! // Install once at boot:
//! rustasea_activitylog::install(Arc::new(ActivityLogger::new(pool.clone())));
//! // Auth middleware (optional): record the actor.
//! rustasea_activitylog::set_causer_resolver(Arc::new(|| current_user_id()));
//!
//! // Query the trail:
//! let logger = ActivityLogger::new(pool.clone());
//! let rows = logger.for_subject("User", user_id).await?;
//! ```
//!
//! ## Causer / CLI-queue safety
//!
//! The causer is resolved from a request-scoped closure. CLI and queue
//! contexts never install one, so the causer is `None` and the row stays
//! null-safe.

mod error;
mod event;
mod global;
mod migration;
mod model;
mod recorder;

#[cfg(test)]
mod tests;

pub use error::{ActivityError, Result};
pub use event::{
    ActivityColumnMode, ActivityColumns, ActivityEvent, ActivityLogError, ActivityOperation,
    ActivityRecorder,
};
pub use global::{
    batch_scope, clear_causer_resolver, current_batch_uuid, current_causer, install,
    set_batch_uuid, set_causer_resolver, uninstall,
};
pub use migration::{register, CreateAuditLogTable};
pub use model::{Activity, ActivityQuery};
pub use recorder::ActivityLogger;

/// Default `audit_log` table name.
pub const AUDIT_LOG_TABLE: &str = "audit_log";
