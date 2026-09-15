//! Model-change audit hooks — recorder registry, event shape, causer/batch seams.
//!
//! This module is the ORM-side seam for the activity-log feature. It stays free
//! of any dependency on the `rustasea-activitylog` crate: the ORM defines the
//! [`ActivityRecorder`] trait and a process-wide slot, and the activity-log
//! crate installs an implementation at application boot. When no recorder is
//! registered the write path pays only a single `Option` read per mutation
//! ([`activity_recorder`]), so opting out is effectively zero-overhead.
//!
//! Three process-wide slots are exposed, each mirroring the global-scope
//! registry's poisoned-lock degradation (never panic):
//!
//! * [`register_activity_recorder`] / [`activity_recorder`] — the recorder.
//! * [`set_causer_resolver`] / [`current_causer`] — the request-scoped causer.
//!   Auth middleware installs a resolver; CLI/queue contexts leave it unset, so
//!   the causer is `None` and the audit row stays null-safe.
//! * [`set_batch_uuid`] / [`current_batch_uuid`] — groups a multi-model write
//!   (e.g. a cascade delete) under one `batch_uuid`.

use std::sync::{Arc, OnceLock, RwLock};

use serde::{Deserialize, Serialize};

/// Boxed error surfaced by an [`ActivityRecorder`] implementation.
///
/// Kept erased so the ORM never depends on the activity-log crate's error type;
/// the write path wraps it as [`crate::error::OrmError::Activity`].
pub type ActivityLogError = Box<dyn std::error::Error + Send + Sync>;

/// The kind of change an activity row records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityOperation {
    /// A row was inserted.
    Created,
    /// A row was updated.
    Updated,
    /// A row was deleted (hard or soft).
    Deleted,
    /// A soft-deleted row was restored.
    Restored,
    /// A row was upserted without a known prior state.
    Saved,
}

impl ActivityOperation {
    /// The stable string form persisted in the activity payload.
    pub fn as_str(&self) -> &'static str {
        match self {
            ActivityOperation::Created => "created",
            ActivityOperation::Updated => "updated",
            ActivityOperation::Deleted => "deleted",
            ActivityOperation::Restored => "restored",
            ActivityOperation::Saved => "saved",
        }
    }
}

/// A single model-change event handed to an [`ActivityRecorder`].
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityEvent {
    /// The kind of change.
    pub operation: ActivityOperation,
    /// The model's table name (subject table).
    pub table: String,
    /// The model's type name (polymorphic `subject_type`).
    pub model_type: String,
    /// The model's primary key (`subject_id`), when known.
    pub model_id: Option<String>,
    /// The attributes before the change, when available.
    pub old: Option<serde_json::Value>,
    /// The attributes after the change, when available.
    pub new: Option<serde_json::Value>,
    /// Names of the attributes whose values changed.
    pub changed: Vec<String>,
    /// The causer (actor) id from the request-scoped resolver, when set.
    pub causer_id: Option<String>,
    /// The batch id grouping this event with related writes, when set.
    pub batch_uuid: Option<String>,
}

impl ActivityEvent {
    /// Build an event for `model_type` with the given operation and id.
    ///
    /// The diff sides, causer, and batch default to empty; chain
    /// [`ActivityEvent::with_diff`], [`ActivityEvent::caused_by`], and
    /// [`ActivityEvent::in_batch`] to fill them in.
    pub fn new(
        operation: ActivityOperation,
        table: impl Into<String>,
        model_type: impl Into<String>,
        model_id: Option<String>,
    ) -> Self {
        Self {
            operation,
            table: table.into(),
            model_type: model_type.into(),
            model_id,
            old: None,
            new: None,
            changed: Vec::new(),
            causer_id: None,
            batch_uuid: None,
        }
    }

    /// Attach the old/new snapshots and the changed-column list.
    pub fn with_diff(
        mut self,
        old: Option<serde_json::Value>,
        new: Option<serde_json::Value>,
        changed: Vec<String>,
    ) -> Self {
        self.old = old;
        self.new = new;
        self.changed = changed;
        self
    }

    /// Attach the causer id.
    pub fn caused_by(mut self, causer_id: impl Into<String>) -> Self {
        self.causer_id = Some(causer_id.into());
        self
    }

    /// Attach a batch id.
    pub fn in_batch(mut self, batch_uuid: impl Into<String>) -> Self {
        self.batch_uuid = Some(batch_uuid.into());
        self
    }
}

/// Which columns of a model participate in the activity log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityColumnMode {
    /// Log every user column.
    All,
    /// Log only the named columns.
    Only(Vec<String>),
    /// Log every user column except the named ones.
    Except(Vec<String>),
}

/// Column-selection policy attached to a model via `#[logs_activity(...)]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityColumns {
    /// The selection mode.
    pub mode: ActivityColumnMode,
}

impl ActivityColumns {
    /// Log every user column.
    pub fn all() -> Self {
        Self {
            mode: ActivityColumnMode::All,
        }
    }

    /// Log only the named columns.
    pub fn only(columns: Vec<String>) -> Self {
        Self {
            mode: ActivityColumnMode::Only(columns),
        }
    }

    /// Log every column except the named ones.
    pub fn except(columns: Vec<String>) -> Self {
        Self {
            mode: ActivityColumnMode::Except(columns),
        }
    }

    /// Whether `column` is allowed to appear in the logged attributes.
    pub fn allows(&self, column: &str) -> bool {
        match &self.mode {
            ActivityColumnMode::All => true,
            ActivityColumnMode::Only(columns) => columns.iter().any(|c| c == column),
            ActivityColumnMode::Except(columns) => !columns.iter().any(|c| c == column),
        }
    }
}

impl Default for ActivityColumns {
    /// Default to logging every column.
    fn default() -> Self {
        Self::all()
    }
}

/// Filter a serialized model object to the columns allowed by `columns`.
///
/// Non-object values pass through unchanged; a redacted column is dropped
/// entirely so it can never leak into the persisted properties.
pub fn filter_columns(object: &serde_json::Value, columns: &ActivityColumns) -> serde_json::Value {
    match object {
        serde_json::Value::Object(map) => {
            let filtered = map
                .iter()
                .filter(|(key, _)| columns.allows(key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            serde_json::Value::Object(filtered)
        }
        other => other.clone(),
    }
}

/// The top-level keys of a JSON object, in insertion order.
pub fn object_keys(object: &serde_json::Value) -> Vec<String> {
    match object {
        serde_json::Value::Object(map) => map.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

/// Names of the top-level keys whose values differ between `old` and `new`.
///
/// The comparison is shallow (Laravel's `getDirty` semantics): a key present in
/// either object is "changed" when its value differs or it is missing on one
/// side.
pub fn changed_columns(old: &serde_json::Value, new: &serde_json::Value) -> Vec<String> {
    let (Some(old_map), Some(new_map)) = (old.as_object(), new.as_object()) else {
        return Vec::new();
    };
    let mut changed = Vec::new();
    for (key, old_value) in old_map {
        match new_map.get(key) {
            Some(new_value) if new_value == old_value => {}
            _ => changed.push(key.clone()),
        }
    }
    for key in new_map.keys() {
        if !old_map.contains_key(key) {
            changed.push(key.clone());
        }
    }
    changed
}

/// Async sink for model-change events.
///
/// Implemented by the activity-log crate's `ActivityLogger` and registered
/// process-wide via [`register_activity_recorder`]. Implementations must be
/// `Send + Sync` so they can be shared across the async write path.
#[async_trait::async_trait]
pub trait ActivityRecorder: Send + Sync {
    /// Persist one activity event.
    async fn record(&self, event: ActivityEvent) -> Result<(), ActivityLogError>;
}

/// Process-wide recorder slot, initialised on first use.
static RECORDER: OnceLock<RwLock<Option<Arc<dyn ActivityRecorder>>>> = OnceLock::new();

/// The recorder slot, initialised to empty on first use.
fn recorder_slot() -> &'static RwLock<Option<Arc<dyn ActivityRecorder>>> {
    RECORDER.get_or_init(|| RwLock::new(None))
}

/// Install `recorder` as the process-wide activity recorder.
///
/// Call once from application boot; a later call replaces the previous
/// recorder. A poisoned lock is recovered rather than surfaced, so a panic in
/// another thread cannot permanently disable auditing.
pub fn register_activity_recorder(recorder: Arc<dyn ActivityRecorder>) {
    let slot = recorder_slot();
    let mut guard = slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = Some(recorder);
}

/// Remove the process-wide activity recorder (bootstrap/test reset hook).
pub fn clear_activity_recorder() {
    let slot = recorder_slot();
    let mut guard = slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = None;
}

/// The installed activity recorder, or `None` when auditing is disabled.
///
/// The write path checks this first and returns immediately when it is `None`,
/// so a model that does not opt in pays no serialization or pre-read cost.
pub fn activity_recorder() -> Option<Arc<dyn ActivityRecorder>> {
    let slot = recorder_slot();
    let guard = slot
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.clone()
}

/// The request-scoped causer resolver type.
pub type CauserResolver = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// Process-wide causer resolver slot.
static CAUSER: OnceLock<RwLock<Option<CauserResolver>>> = OnceLock::new();

/// The causer slot, initialised to empty on first use.
fn causer_slot() -> &'static RwLock<Option<CauserResolver>> {
    CAUSER.get_or_init(|| RwLock::new(None))
}

/// Install the request-scoped causer resolver.
///
/// Auth middleware calls this with a closure reading the authenticated user's
/// id. CLI/queue contexts leave it unset, so [`current_causer`] returns `None`
/// and the audit row records no actor (null-safe).
pub fn set_causer_resolver(resolver: CauserResolver) {
    let slot = causer_slot();
    let mut guard = slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = Some(resolver);
}

/// Remove the causer resolver (bootstrap/test reset hook).
pub fn clear_causer_resolver() {
    let slot = causer_slot();
    let mut guard = slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = None;
}

/// Resolve the current causer id, or `None` when no resolver is installed.
pub fn current_causer() -> Option<String> {
    let slot = causer_slot();
    let guard = slot
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.as_ref().and_then(|resolver| resolver())
}

/// Process-wide batch-uuid slot.
static BATCH: OnceLock<RwLock<Option<String>>> = OnceLock::new();

/// The batch slot, initialised to empty on first use.
fn batch_slot() -> &'static RwLock<Option<String>> {
    BATCH.get_or_init(|| RwLock::new(None))
}

/// Set (or clear, with `None`) the batch id stamped on subsequent events.
///
/// Set before a multi-model operation and clear it afterwards — or use
/// [`BatchScope`], which clears on drop.
pub fn set_batch_uuid(batch_uuid: Option<String>) {
    let slot = batch_slot();
    let mut guard = slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = batch_uuid;
}

/// The current batch id, or `None` when no batch is open.
pub fn current_batch_uuid() -> Option<String> {
    let slot = batch_slot();
    let guard = slot
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.clone()
}

/// RAII guard that clears the batch id on drop.
///
/// The batch id must be a valid UUID string, since the activity log stores it in
/// a UUID column; an invalid value is rejected with a typed error at record time.
///
/// ```rust,ignore
/// let _batch = BatchScope::new("018f2e5c-1a2b-7000-8000-000000000000");
/// // ... related writes share batch_uuid = "018f2e5c-..." ...
/// // dropped here: the slot is cleared
/// ```
pub struct BatchScope;

impl BatchScope {
    /// Open a batch scope with `batch_uuid`, clearing it on drop.
    pub fn new(batch_uuid: impl Into<String>) -> Self {
        set_batch_uuid(Some(batch_uuid.into()));
        Self
    }
}

impl Drop for BatchScope {
    fn drop(&mut self) {
        set_batch_uuid(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies operation names round-trip through their stable string form.
    #[test]
    fn operation_names_are_stable() {
        assert_eq!(ActivityOperation::Created.as_str(), "created");
        assert_eq!(ActivityOperation::Updated.as_str(), "updated");
        assert_eq!(ActivityOperation::Deleted.as_str(), "deleted");
        assert_eq!(ActivityOperation::Restored.as_str(), "restored");
        assert_eq!(ActivityOperation::Saved.as_str(), "saved");
    }

    /// Verifies `All` allows every column and `Only`/`Except` gate correctly.
    #[test]
    fn column_modes_gate_columns() {
        assert!(ActivityColumns::all().allows("password"));
        let only = ActivityColumns::only(vec!["name".into(), "email".into()]);
        assert!(only.allows("name"));
        assert!(!only.allows("password"));
        let except = ActivityColumns::except(vec!["password".into()]);
        assert!(!except.allows("password"));
        assert!(except.allows("name"));
    }

    /// Verifies filtering drops redacted columns entirely.
    #[test]
    fn filter_columns_redacts() {
        let object = serde_json::json!({ "name": "Ada", "password": "secret" });
        let filtered = filter_columns(&object, &ActivityColumns::except(vec!["password".into()]));
        assert_eq!(filtered, serde_json::json!({ "name": "Ada" }));
    }

    /// Verifies a shallow diff reports added, removed, and changed keys.
    #[test]
    fn changed_columns_is_shallow() {
        let old = serde_json::json!({ "name": "Ada", "email": "a@x.test" });
        let new =
            serde_json::json!({ "name": "Ada Lovelace", "email": "a@x.test", "role": "admin" });
        let mut changed = changed_columns(&old, &new);
        changed.sort();
        assert_eq!(changed, vec!["name".to_string(), "role".to_string()]);
        assert!(changed_columns(&old, &old).is_empty());
    }

    /// Verifies the registry round-trips an installed recorder.
    #[test]
    fn recorder_registry_round_trips() {
        struct Noop;
        #[async_trait::async_trait]
        impl ActivityRecorder for Noop {
            async fn record(&self, _event: ActivityEvent) -> Result<(), ActivityLogError> {
                Ok(())
            }
        }
        clear_activity_recorder();
        assert!(activity_recorder().is_none());
        register_activity_recorder(Arc::new(Noop));
        assert!(activity_recorder().is_some());
        clear_activity_recorder();
        assert!(activity_recorder().is_none());
    }

    /// Verifies the causer and batch slots round-trip.
    #[test]
    fn causer_and_batch_slots() {
        clear_causer_resolver();
        assert_eq!(current_causer(), None);
        set_causer_resolver(Arc::new(|| Some("user-1".to_string())));
        assert_eq!(current_causer(), Some("user-1".to_string()));
        clear_causer_resolver();
        assert_eq!(current_causer(), None);

        set_batch_uuid(Some("batch-1".to_string()));
        assert_eq!(current_batch_uuid(), Some("batch-1".to_string()));
        {
            let _scope = BatchScope::new("batch-2");
            assert_eq!(current_batch_uuid(), Some("batch-2".to_string()));
        }
        assert_eq!(current_batch_uuid(), None);
    }
}
