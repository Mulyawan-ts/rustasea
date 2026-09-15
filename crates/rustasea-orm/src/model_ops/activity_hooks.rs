//! Audit hooks for the write path — build [`ActivityEvent`]s and dispatch them.
//!
//! Split out of [`super`] to keep the write path within the file-size standard.
//! Every helper is `pub(crate)` and gated on the model opting in
//! ([`Model::logs_activity`]) **and** a recorder being installed, so a model
//! that does not opt in pays no serialization, pre-read, or snapshot cost.
//!
//! Snapshots are the raw database row (filtered to the model's
//! [`Model::activity_columns`] policy), so the old/new diff and the `changed`
//! list describe exactly what the database held — a json-cast column cannot
//! produce a spurious "changed" from a representation mismatch.

use std::sync::Arc;

use crate::activity::{
    activity_recorder, changed_columns, current_batch_uuid, current_causer, filter_columns,
    object_keys, ActivityEvent, ActivityOperation, ActivityRecorder,
};
use crate::db::DbPool;
use crate::error::{OrmError, Result};
use crate::model::Model;
use uuid::Uuid;

/// The recorder to use for `T`, or `None` when auditing is off for the model.
///
/// Checks the model's opt-in flag first, so a non-opted model never even reads
/// the process-wide recorder slot.
pub(crate) fn recorder<T: Model>() -> Option<Arc<dyn ActivityRecorder>> {
    if T::logs_activity() {
        activity_recorder()
    } else {
        None
    }
}

/// Read the current row as a column-filtered JSON snapshot, including trashed rows.
///
/// Returns `None` when the row does not exist. Used for the "before" side of a
/// change; the "after" side is supplied by the caller from the row it already
/// re-read after the mutation, so the write path never issues a second SELECT.
pub(crate) async fn snapshot<T: Model>(
    pool: &DbPool,
    id: Uuid,
) -> Result<Option<serde_json::Value>> {
    let row = <T as Model>::query_with_trashed()
        .where_key(id)
        .first(pool)
        .await?;
    Ok(row.map(|row| filter_columns(&row, &T::activity_columns())))
}

/// Re-read the mutated row once and split it into the hydrated model and the
/// column-filtered activity snapshot.
///
/// The write path reuses this single SELECT for both the returned model and the
/// recorded event, so auditing adds no post-mutation round-trip. `fallback` is
/// returned as the model when the row vanished between the write and the read.
pub(crate) async fn refresh_with_snapshot<T>(
    pool: &DbPool,
    id: Uuid,
    recorder: Option<&Arc<dyn ActivityRecorder>>,
    fallback: T,
) -> Result<(T, Option<serde_json::Value>)>
where
    T: Model + serde::de::DeserializeOwned,
{
    let raw = super::refresh_raw::<T>(pool, id).await?;
    let snapshot = match (recorder, &raw) {
        (Some(_), Some(row)) => Some(filter_columns(row, &T::activity_columns())),
        _ => None,
    };
    let persisted = match raw {
        Some(value) => crate::casts::hydrate::<T>(value)?,
        None => fallback,
    };
    Ok((persisted, snapshot))
}

/// The `changed` list for an update-style event (shallow diff, or all user
/// columns when there is no prior snapshot).
///
/// ORM-managed columns (`id`, `created_at`, `updated_at`, `deleted_at`) are
/// excluded: they are not user attributes, and a no-op update still bumps
/// `updated_at`, so including it would defeat `log_only_dirty`.
fn diff_changed(old: Option<&serde_json::Value>, new: Option<&serde_json::Value>) -> Vec<String> {
    let columns = match (old, new) {
        (Some(old), Some(new)) => changed_columns(old, new),
        (None, Some(new)) => object_keys(new),
        _ => Vec::new(),
    };
    columns
        .into_iter()
        .filter(|name| is_user_column(name))
        .collect()
}

/// Whether `name` is a user attribute (not an ORM-managed column).
fn is_user_column(name: &str) -> bool {
    !matches!(name, "id" | "created_at" | "updated_at" | "deleted_at")
}

/// Assemble an event for `T` with the request-scoped causer and batch id.
fn event<T: Model>(
    operation: ActivityOperation,
    id: Uuid,
    old: Option<serde_json::Value>,
    new: Option<serde_json::Value>,
    changed: Vec<String>,
) -> ActivityEvent {
    ActivityEvent {
        operation,
        table: T::table_name(),
        model_type: T::type_name().to_string(),
        model_id: Some(id.to_string()),
        old,
        new,
        changed,
        causer_id: current_causer(),
        batch_uuid: current_batch_uuid(),
    }
}

/// Record a create event for the just-inserted row.
///
/// `new` is the post-insert snapshot captured by the write path from the row it
/// already re-read, so no second SELECT is issued. The `changed` list is the
/// snapshot's user columns (ORM-managed columns are excluded, matching
/// [`diff_changed`]).
pub(crate) async fn record_created<T: Model>(
    recorder: &Arc<dyn ActivityRecorder>,
    id: Uuid,
    new: Option<serde_json::Value>,
) -> Result<()> {
    let changed = new
        .as_ref()
        .map(object_keys)
        .unwrap_or_default()
        .into_iter()
        .filter(|name| is_user_column(name))
        .collect();
    let event = event::<T>(ActivityOperation::Created, id, None, new, changed);
    dispatch(recorder, event).await
}

/// Record an update/upsert event, skipping a no-op when `only_dirty` is set.
///
/// `new` is the post-mutation snapshot captured by the write path from the row
/// it already re-read, so no second SELECT is issued. Returns `Ok(true)` when a
/// row was recorded, `Ok(false)` when it was skipped (no-op with `only_dirty`).
pub(crate) async fn record_updated<T: Model>(
    recorder: &Arc<dyn ActivityRecorder>,
    id: Uuid,
    old: Option<serde_json::Value>,
    new: Option<serde_json::Value>,
    operation: ActivityOperation,
) -> Result<bool> {
    let changed = diff_changed(old.as_ref(), new.as_ref());
    if changed.is_empty() && T::activity_log_only_dirty() {
        return Ok(false);
    }
    let event = event::<T>(operation, id, old, new, changed);
    dispatch(recorder, event).await?;
    Ok(true)
}

/// Record a delete event with the pre-delete snapshot (and the post-delete row
/// for a soft delete, where the row still exists with `deleted_at` set).
pub(crate) async fn record_deleted<T: Model>(
    recorder: &Arc<dyn ActivityRecorder>,
    id: Uuid,
    old: Option<serde_json::Value>,
    new: Option<serde_json::Value>,
) -> Result<()> {
    let changed = diff_changed(old.as_ref(), new.as_ref());
    let event = event::<T>(ActivityOperation::Deleted, id, old, new, changed);
    dispatch(recorder, event).await
}

/// Hand an event to the recorder, mapping a recorder failure to a typed error.
///
/// Audit integrity wins over write availability: a failed recording is surfaced
/// as [`OrmError::Activity`] rather than silently swallowed.
pub(crate) async fn dispatch(
    recorder: &Arc<dyn ActivityRecorder>,
    event: ActivityEvent,
) -> Result<()> {
    recorder
        .record(event)
        .await
        .map_err(|error| OrmError::Activity(error.to_string()))
}
