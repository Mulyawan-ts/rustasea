//! Declarative cascade soft delete / restore / force delete (ADOPT-018).
//!
//! When a model opts in with `#[cascade_soft_deletes("posts", ...)]`, its
//! [`ModelOps`](super::ModelOps) delete/restore path also applies the operation
//! to the rows of every named relation. The parent's own statement runs first
//! (so a soft delete stamps the parent before its children), then this module
//! fans the same operation out to the related tables in chunks.
//!
//! # Single-level semantics
//!
//! Cascade is intentionally **single level**: only the relations the parent
//! declares are affected. A child's own `cascade_soft_deletes` declaration is
//! *not* followed, because the child model type is not available here — the
//! related rows are addressed by raw SQL over `relation.related_table`, and the
//! ORM has no process-wide model registry to resolve a table back to a type.
//! Multi-level cascade would require such a registry (out of scope); the
//! primary use case (one parent, one declared fan-out) is covered.
//!
//! # Restore provenance guard
//!
//! Restore only cascades to children that were soft-deleted **at or after** the
//! parent's own `deleted_at`. A row deleted independently, earlier than the
//! parent, keeps its earlier stamp and is therefore *not* resurrected. The
//! comparison is a lexicographic RFC3339 string compare over the JSON rows
//! returned by every driver, so it behaves the same on SQLite/Postgres/MySQL.
//!
//! # Child primary key
//!
//! The child primary-key column is assumed to be `id` (a single UUID), the ORM
//! default. Composite-key children are not cascaded by this module.
//!
//! # Identifier safety
//!
//! The child table name and every key column a relation interpolates into
//! `UPDATE` / `DELETE` / `IN` are validated as `^[A-Za-z_][A-Za-z0-9_]*$` before
//! any SQL is built. A relation carrying anything else fails with a typed
//! [`OrmError::InvalidState`] instead of splicing the fragment into a statement.
//!
//! # Activity log
//!
//! When the parent opts into the activity log, one event per affected child is
//! dispatched with `model_type` set to the related table name (the child's Rust
//! type is unknown here). Every event — parent and children — shares the
//! process-wide `batch_uuid` opened by the caller's [`BatchScope`].
//!
//! # Atomicity
//!
//! The parent statement and the child fan-out run as separate autocommit
//! statements, **not** inside one transaction. A transaction would have to
//! thread a single [`Transaction`](crate::tx::Transaction) handle through the
//! whole engine *and* the activity snapshot reads, which run on the pool; on a
//! single-connection SQLite pool that self-deadlocks (the documented
//! non-nesting invariant in [`crate::tx`]). Errors are therefore propagated so
//! the caller sees a typed failure, but a mid-cascade error can leave the
//! parent and some children mutated. Callers that need all-or-nothing semantics
//! should wrap the call in their own transaction once a transaction-aware
//! cascade engine lands.
//!
//! [`BatchScope`]: crate::activity::BatchScope

use std::collections::HashSet;

use chrono::Utc;

use crate::activity::{current_batch_uuid, current_causer, ActivityEvent, ActivityOperation};
use crate::builder::QueryBuilder;
use crate::db::DbPool;
use crate::eager::{key_to_value, parent_keys, parent_rows, row_key};
use crate::error::{OrmError, Result};
use crate::model::{Model, Relation, RelationKind};
use crate::relations::MAX_DEPTH;
use crate::types::Value;

use super::activity_hooks::{dispatch, recorder};
use super::key::KeyFilter;
use super::timestamps::timestamp_value;

/// Maximum number of child ids bound into a single `IN (...)` statement.
///
/// A large fan-out is split into bounded batches so the placeholder list never
/// grows without limit (mirrors Laravel's `chunkById` batching).
pub(crate) const CASCADE_CHUNK_SIZE: usize = 500;

/// The operation being cascaded from a parent to its declared relations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CascadeMode {
    /// Set `deleted_at` on active children (`deleted_at IS NULL`).
    SoftDelete,
    /// Clear `deleted_at` on children deleted at/after the parent.
    Restore,
    /// Permanently remove the children.
    ForceDelete,
}

impl CascadeMode {
    /// The activity operation a child event records for this mode.
    fn operation(self) -> ActivityOperation {
        match self {
            CascadeMode::SoftDelete | CascadeMode::ForceDelete => ActivityOperation::Deleted,
            CascadeMode::Restore => ActivityOperation::Restored,
        }
    }
}

/// Cascade `mode` from `parent` (a pre-mutation row snapshot) to its relations.
///
/// The caller captures `parent` **before** mutating it, because a force delete
/// removes the row and a restore clears its `deleted_at` — both would hide the
/// foreign-key values and the original stamp this fan-out depends on. `depth`
/// guards against exceeding [`MAX_DEPTH`]. `visited` dedupes relation **names**
/// (two siblings may share a child table yet each cascades once) and a relation
/// pointing back at the parent's own table is skipped, so a self-referential
/// declaration never cascades into the parent's own rows.
pub(crate) async fn cascade<T: Model>(
    pool: &DbPool,
    parent: &serde_json::Value,
    mode: CascadeMode,
    depth: usize,
    visited: &mut HashSet<String>,
) -> Result<()> {
    if depth >= MAX_DEPTH {
        return Ok(());
    }
    // The restore guard needs the parent's pre-restore stamp.
    let parent_deleted_at = parent
        .get("deleted_at")
        .and_then(|value| value.as_str())
        .map(str::to_string);

    let declared = T::relations();
    let child_recorder = recorder::<T>();
    for name in T::cascade_relations() {
        let relation = declared.iter().find(|relation| relation.name == *name);
        let Some(relation) = relation else {
            return Err(OrmError::InvalidState(format!(
                "cascade relation `{name}` is not declared on {}",
                T::type_name()
            )));
        };
        // A BelongsTo points the wrong way (the parent holds the FK); skip it.
        if relation.kind == RelationKind::BelongsTo {
            continue;
        }
        // Defense-in-depth: never interpolate an unvalidated identifier.
        validate_relation_identifiers(relation)?;
        // A relation that targets the parent's own table would cascade the
        // parent's own rows (single-level cascade never follows it back).
        if relation.related_table == T::table_name() {
            continue;
        }
        // Dedupe by relation name, not table: two siblings may target the same
        // child table with different foreign keys and must each cascade once.
        if !visited.insert(relation.name.clone()) {
            continue;
        }
        if mode == CascadeMode::Restore && parent_deleted_at.is_none() {
            // Nothing was soft-deleted on the parent, so nothing to cascade.
            continue;
        }

        let rows = child_rows(parent, relation, mode, pool).await?;
        let rows = match mode {
            CascadeMode::Restore => retain_restorable(rows, parent_deleted_at.as_deref()),
            _ => rows,
        };
        let ids: Vec<Value> = rows
            .iter()
            .filter_map(|row| row_key(row, "id"))
            .map(|key| key_to_value(&key))
            .collect();

        for chunk in ids.chunks(CASCADE_CHUNK_SIZE) {
            execute_child_chunk(pool, &relation.related_table, chunk, mode).await?;
        }
        if let Some(recorder) = &child_recorder {
            for row in &rows {
                record_child(recorder, relation, row, mode).await?;
            }
        }
    }
    Ok(())
}

/// Read the parent row addressed by `key`, including soft-deleted rows.
///
/// The caller captures this before mutating the parent so the cascade fan-out
/// can still see the foreign-key values and the pre-mutation `deleted_at`.
pub(crate) async fn parent_snapshot<T: Model>(
    pool: &DbPool,
    key: &KeyFilter,
) -> Result<Option<serde_json::Value>> {
    key.apply(<T as Model>::query_with_trashed())
        .first(pool)
        .await
}

/// Query the child rows of `relation` for the given `parent`, honoring `mode`.
///
/// Soft delete touches only active children (`deleted_at IS NULL`); restore
/// starts from the trashed set (`deleted_at IS NOT NULL`) before the provenance
/// guard; force delete ignores the marker entirely.
async fn child_rows(
    parent: &serde_json::Value,
    relation: &crate::model::Relation,
    mode: CascadeMode,
    pool: &DbPool,
) -> Result<Vec<serde_json::Value>> {
    let table = relation.related_table.clone();
    let mut query = QueryBuilder::table(table);
    if relation.is_composite() {
        let foreign_keys: Vec<&str> = relation
            .effective_foreign_keys()
            .iter()
            .map(String::as_str)
            .collect();
        // Match the child's foreign-key tuple against the parent's local-key
        // tuple (e.g. child `(tenant_id, user_id)` vs parent `(tenant_id, id)`).
        let local_keys: Vec<String> = relation.effective_local_keys().to_vec();
        let tuples = parent_rows(std::slice::from_ref(parent), &local_keys);
        query = query.where_in_rows(&foreign_keys, tuples);
    } else {
        // Match the child's foreign key against the parent's local key value.
        let keys = parent_keys(std::slice::from_ref(parent), &relation.local_key);
        query = query.where_in(&relation.foreign_key, keys);
    }
    query = match mode {
        CascadeMode::SoftDelete => query.where_null("deleted_at"),
        CascadeMode::Restore => query.where_not_null("deleted_at"),
        CascadeMode::ForceDelete => query,
    };
    query.get(pool).await
}

/// Keep only child rows whose `deleted_at` is at or after the parent's stamp.
///
/// This is the restore provenance guard: a row deleted independently, before
/// the parent, has an earlier stamp and is dropped from the restore set.
fn retain_restorable(
    rows: Vec<serde_json::Value>,
    parent_deleted_at: Option<&str>,
) -> Vec<serde_json::Value> {
    let Some(parent_stamp) = parent_deleted_at else {
        return Vec::new();
    };
    rows.into_iter()
        .filter(|row| {
            row.get("deleted_at")
                .and_then(|value| value.as_str())
                .is_some_and(|stamp| stamp >= parent_stamp)
        })
        .collect()
}

/// Validate every identifier a relation interpolates into child SQL.
///
/// The child table name and its key columns (foreign keys and the `id` primary
/// key) are spliced into `UPDATE` / `DELETE` / `IN` fragments. Each must match
/// `^[A-Za-z_][A-Za-z0-9_]*$`; anything else is a typed
/// [`OrmError::InvalidState`] raised before any statement is built.
fn validate_relation_identifiers(relation: &Relation) -> Result<()> {
    validate_identifier("related table", &relation.related_table)?;
    for key in relation.effective_foreign_keys() {
        validate_identifier("foreign key", key)?;
    }
    for key in relation.effective_local_keys() {
        validate_identifier("local key", key)?;
    }
    Ok(())
}

/// Whether `name` is a safe SQL identifier (`^[A-Za-z_][A-Za-z0-9_]*$`).
///
/// Mirrors the CLI tinker verb guard so the same rule protects every
/// interpolation point.
fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Validate one interpolated identifier, returning a typed error otherwise.
fn validate_identifier(label: &str, name: &str) -> Result<()> {
    if is_valid_identifier(name) {
        return Ok(());
    }
    Err(OrmError::InvalidState(format!(
        "cascade {label} `{name}` is not a valid SQL identifier"
    )))
}

/// Apply `mode` to one chunk of child ids with a single statement.
///
/// SQLite binds an RFC3339 `deleted_at` string for a soft delete; Postgres/MySQL
/// use the native `NOW()`. Restore and force delete need no dialect branch. The
/// `table` is validated here as well, at the exact interpolation point, so a
/// future caller cannot splice an unvalidated identifier into `UPDATE`/`DELETE`.
/// A successful statement bumps the child table's cache generation, so a cached
/// query on that child table misses on the next read (ADOPT-019).
async fn execute_child_chunk(
    pool: &DbPool,
    table: &str,
    ids: &[Value],
    mode: CascadeMode,
) -> Result<()> {
    validate_identifier("related table", table)?;
    let mut bindings: Vec<Value> = Vec::with_capacity(ids.len() + 1);
    let placeholders: Vec<String> = ids
        .iter()
        .map(|id| {
            bindings.push(id.clone());
            format!("${}", bindings.len())
        })
        .collect();
    let list = placeholders.join(", ");
    let sql = match mode {
        CascadeMode::SoftDelete => {
            let expr = match pool.dialect() {
                "sqlite" => {
                    bindings.push(timestamp_value(Utc::now(), "sqlite"));
                    format!("${}", bindings.len())
                }
                _ => "NOW()".to_string(),
            };
            format!("UPDATE {table} SET deleted_at = {expr} WHERE id IN ({list})")
        }
        CascadeMode::Restore => {
            format!("UPDATE {table} SET deleted_at = NULL WHERE id IN ({list})")
        }
        CascadeMode::ForceDelete => format!("DELETE FROM {table} WHERE id IN ({list})"),
    };
    pool.execute_bind(&sql, &bindings).await?;
    super::cache_hooks::invalidate_table(table);
    Ok(())
}

/// Dispatch one child cascade event under the caller's shared `batch_uuid`.
///
/// The child's Rust type is unknown, so `model_type` is the related table name;
/// the causer and batch id come from the same process-wide slots as the parent.
async fn record_child(
    recorder: &std::sync::Arc<dyn crate::activity::ActivityRecorder>,
    relation: &crate::model::Relation,
    row: &serde_json::Value,
    mode: CascadeMode,
) -> Result<()> {
    let model_id = row_key(row, "id");
    let event = ActivityEvent {
        operation: mode.operation(),
        table: relation.related_table.clone(),
        model_type: relation.related_table.clone(),
        model_id,
        old: Some(row.clone()),
        new: None,
        changed: Vec::new(),
        causer_id: current_causer(),
        batch_uuid: current_batch_uuid(),
    };
    dispatch(recorder, event).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies the identifier rule accepts the canonical shapes.
    #[test]
    fn identifier_rule_accepts_safe_names() {
        assert!(is_valid_identifier("posts"));
        assert!(is_valid_identifier("_private"));
        assert!(is_valid_identifier("posts2"));
        assert!(is_valid_identifier("archived_by"));
    }

    /// Verifies the identifier rule rejects anything unsafe.
    #[test]
    fn identifier_rule_rejects_unsafe_names() {
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("1bad"));
        assert!(!is_valid_identifier("user table"));
        assert!(!is_valid_identifier("users;"));
        assert!(!is_valid_identifier("posts; DROP TABLE users"));
    }

    /// Verifies a relation with a hostile table name is a typed error.
    #[test]
    fn hostile_table_name_is_typed_error() {
        let relation = Relation::has_many("posts", "posts; DROP TABLE users", "User");
        let error = validate_relation_identifiers(&relation)
            .expect_err("hostile table name must be rejected");
        match error {
            OrmError::InvalidState(message) => {
                assert!(message.contains("related table"), "{message}");
                assert!(message.contains("posts; DROP"), "{message}");
            }
            other => panic!("expected InvalidState, got {other:?}"),
        }
    }

    /// Verifies a relation with a hostile key column is a typed error.
    #[test]
    fn hostile_key_column_is_typed_error() {
        let relation = Relation::has_many_composite("posts", "posts", &["user_id; DROP"], &["id"])
            .expect("constructs a relation; validation is separate");
        let error = validate_relation_identifiers(&relation)
            .expect_err("hostile key column must be rejected");
        assert!(matches!(error, OrmError::InvalidState(_)), "got {error:?}");
    }

    /// Verifies a well-formed relation passes validation.
    #[test]
    fn well_formed_relation_passes_validation() {
        let relation = Relation::has_many("posts", "posts", "User");
        assert!(validate_relation_identifiers(&relation).is_ok());
    }
}
