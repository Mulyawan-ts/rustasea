//! Restore path — clear `deleted_at` and cascade the restore to children.
//!
//! Split out of [`super`] to keep the write path within the file-size standard.
//! A restore is the inverse of a soft delete: it sets `deleted_at = NULL` for
//! the addressed row and, when the model opts into cascading, for the children
//! that were soft-deleted together with it (see [`super::cascade`] for the
//! provenance guard that leaves independently-deleted rows untouched).

use std::collections::HashSet;

use uuid::Uuid;

use crate::activity::BatchScope;
use crate::db::DbPool;
use crate::error::Result;
use crate::model::Model;

use super::activity_hooks::{record_restored, recorder, snapshot};
use super::cache_hooks::invalidate_table;
use super::cascade::{cascade, parent_snapshot, CascadeMode};
use super::composite::build_restore;
use super::key::KeyFilter;

/// Restore the row addressed by `filter`, cascading when the model opts in.
///
/// Returns `Ok(true)` when a row was restored, `Ok(false)` when the key matched
/// no row. When [`Model::cascade_soft_deletes`] is true, the parent update and
/// the child fan-out share one [`BatchScope`], so every activity event carries
/// the same `batch_uuid`.
pub(crate) async fn restore_by_key<T: Model>(pool: &DbPool, filter: &KeyFilter) -> Result<bool> {
    let cascading = T::cascade_soft_deletes();
    // The batch groups the parent restore with every cascaded child event.
    let _batch = cascading.then(|| BatchScope::new(Uuid::now_v7().to_string()));
    let recorder = recorder::<T>();
    let old = match &recorder {
        Some(_) => snapshot::<T>(pool, filter).await?,
        None => None,
    };
    // Capture the parent before the restore clears its `deleted_at`, so the
    // provenance guard still sees the original stamp.
    let cascade_parent = if cascading {
        parent_snapshot::<T>(pool, filter).await?
    } else {
        None
    };

    let (sql, bindings) = build_restore::<T>(filter);
    let affected = pool.execute_bind(&sql, &bindings).await?;
    if let Some(recorder) = recorder {
        if affected > 0 {
            let new = snapshot::<T>(pool, filter).await?;
            record_restored::<T>(&recorder, &filter.event_id(), old, new).await?;
        }
    }
    if affected > 0 {
        if let Some(parent) = cascade_parent {
            let mut visited = HashSet::new();
            cascade::<T>(pool, &parent, CascadeMode::Restore, 0, &mut visited).await?;
        }
    }
    invalidate_table(&T::table_name());
    Ok(affected > 0)
}
