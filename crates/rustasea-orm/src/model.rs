//! Model trait, timestamps, soft deletes, and relations.

use crate::builder::{QueryBuilder, Raw};
use crate::casts::CastBinding;
use crate::error::{OrmError, Result};
use crate::execution::count_sql;
use crate::m2::{InsertBuilder, UpsertBuilder};
use crate::naming::snake_plural;
use crate::scopes::{GlobalScopeEntry, SoftDeletesScope};
use crate::types::Value;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Timestamp columns managed by the ORM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timestamps {
    /// Row creation time (`created_at`).
    pub created_at: DateTime<Utc>,
    /// Last update time (`updated_at`), bumped via `set_updated_at()` trigger or ORM.
    pub updated_at: DateTime<Utc>,
}

impl Default for Timestamps {
    /// Default to "now" for both stamps.
    fn default() -> Self {
        let now = Utc::now();
        Self {
            created_at: now,
            updated_at: now,
        }
    }
}

/// Soft delete contract — `deleted_at IS NULL` means active.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SoftDeletes {
    /// Soft-delete marker; `None` = active row.
    pub deleted_at: Option<DateTime<Utc>>,
}

impl SoftDeletes {
    /// Whether the row has been soft-deleted.
    pub fn trashed(&self) -> bool {
        self.deleted_at.is_some()
    }
}

// `Relation`, `RelationKind`, and `JsonSpec` live in [`crate::relation`] (the
// module owns the builder constructors and their validation); they are
// re-exported here so `crate::model::{Relation, RelationKind}` paths stay
// stable for existing callers.
pub use crate::relation::{JsonSpec, Relation, RelationKind};
pub use crate::relations::Relations;

/// Base model contract implemented by `#[derive(Model)]` (macro in a later sprint).
pub trait Model: Send + Sync {
    /// Table name, derived from `snake_plural(type_name)`.
    fn table_name() -> String
    where
        Self: Sized,
    {
        snake_plural(Self::type_name())
    }

    /// Type name used to derive the table name.
    fn type_name() -> &'static str
    where
        Self: Sized;

    /// Primary key value (`id UUID`).
    fn primary_key(&self) -> Uuid;

    /// Create a new UUID before first persistence.
    fn assign_id(&mut self) -> Uuid;

    /// The primary-key column names.
    ///
    /// Defaults to the single `["id"]` column; the derive overrides this for a
    /// `#[model(primary_key = ["tenant_id", "user_id"])]` composite key.
    fn primary_key_columns() -> &'static [&'static str]
    where
        Self: Sized,
    {
        &["id"]
    }

    /// The primary-key bind values in [`Model::primary_key_columns`] order.
    ///
    /// Defaults to `[Value::Uuid(self.primary_key())]` for the single `id`
    /// column. The derive overrides this for a composite key, collecting each
    /// declared column's value; it must stay aligned with
    /// [`Model::primary_key_columns`].
    fn primary_key_values(&self) -> Vec<Value> {
        vec![Value::Uuid(self.primary_key())]
    }

    /// Whether this model keys on more than one column.
    fn has_composite_primary_key() -> bool
    where
        Self: Sized,
    {
        Self::primary_key_columns().len() > 1
    }

    /// Declared relations for eager loading.
    fn relations() -> Vec<Relation>
    where
        Self: Sized,
    {
        Vec::new()
    }

    /// Whether this model cascades soft deletes to its declared relations.
    ///
    /// Opt in with `#[cascade_soft_deletes("posts")]`; names must match
    /// [`Model::relations`]. The default is `false`.
    fn cascade_soft_deletes() -> bool
    where
        Self: Sized,
    {
        false
    }

    /// The relation names whose rows cascade with this model's soft delete,
    /// restore, and force delete (empty when cascading is off).
    fn cascade_relations() -> &'static [&'static str]
    where
        Self: Sized,
    {
        &[]
    }

    /// Per-column attribute casts applied on hydration and persistence.
    ///
    /// `#[derive(Model)]` emits an override for every `#[model(cast = "...")]`
    /// / `#[model(cast_with = "...")]` field; the default is an empty set, so
    /// hand-written models opt in by returning their own bindings.
    fn casts() -> Vec<CastBinding>
    where
        Self: Sized,
    {
        Vec::new()
    }

    /// Whether this model uses `deleted_at` soft deletes.
    fn uses_soft_deletes() -> bool
    where
        Self: Sized,
    {
        true
    }

    /// Whether this model maintains `created_at`/`updated_at`.
    fn uses_timestamps() -> bool
    where
        Self: Sized,
    {
        true
    }

    /// The set of columns the ORM writes on insert (timestamps first).
    ///
    /// Derived models expose their tracked columns through the macro; this
    /// default covers the canonical `created_at`/`updated_at` pair.
    fn insert_columns() -> Vec<&'static str>
    where
        Self: Sized,
    {
        if Self::uses_timestamps() {
            vec!["created_at", "updated_at"]
        } else {
            Vec::new()
        }
    }

    /// The column bumped on update (`updated_at` when tracked).
    fn updated_column() -> Option<&'static str>
    where
        Self: Sized,
    {
        Self::uses_timestamps().then_some("updated_at")
    }

    /// The soft-delete marker column, when the model soft deletes.
    fn deleted_column() -> Option<&'static str>
    where
        Self: Sized,
    {
        Self::uses_soft_deletes().then_some("deleted_at")
    }

    /// Global scopes applied automatically to every query on this model.
    ///
    /// Defaults to [`SoftDeletesScope`] when [`Model::uses_soft_deletes`] is
    /// true; the process-wide registry ([`crate::scopes::register_global_scope`])
    /// is consulted too, so application-registered scopes (e.g. tenancy) join the
    /// model's built-in ones. Bypass them with
    /// [`QueryBuilder::without_global_scopes`] / [`QueryBuilder::without_global_scope`].
    fn global_scopes() -> Vec<GlobalScopeEntry>
    where
        Self: Sized,
    {
        let mut scopes = Vec::new();
        if Self::uses_soft_deletes() {
            scopes.push(GlobalScopeEntry::new(SoftDeletesScope));
        }
        for entry in crate::scopes::registered_global_scopes(&Self::table_name()) {
            if scopes.iter().any(|existing| existing.id() == entry.id()) {
                continue;
            }
            scopes.push(entry);
        }
        scopes
    }

    /// Whether this model's changes are recorded in the activity log.
    ///
    /// Defaults to `false`, so a model that does not opt in never triggers the
    /// audit hooks (zero overhead). `#[derive(Model)]` emits `true` when the
    /// struct carries `#[logs_activity]`; a hand-written impl opts in by
    /// overriding this method.
    fn logs_activity() -> bool
    where
        Self: Sized,
    {
        false
    }

    /// Which columns of this model participate in the activity log.
    ///
    /// Only consulted when [`Model::logs_activity`] is true. The derive emits
    /// `Only`/`Except` for `#[logs_activity(only = "...")]` /
    /// `#[logs_activity(except = "...")]` and `All` otherwise; a hand-written
    /// impl can return its own policy. Skipped columns never appear in the
    /// persisted properties.
    fn activity_columns() -> crate::activity::ActivityColumns
    where
        Self: Sized,
    {
        crate::activity::ActivityColumns::all()
    }

    /// Whether an update that changed nothing should be skipped.
    ///
    /// Defaults to `false` (an empty diff is still recorded). Set `true` via
    /// `#[logs_activity(only_dirty = "true")]` so a no-op update writes no row.
    fn activity_log_only_dirty() -> bool
    where
        Self: Sized,
    {
        false
    }

    /// Whether this model derives a URL-safe slug on write (ADOPT-016).
    ///
    /// Defaults to `false` (zero overhead); `#[derive(Model)]` emits `true` for
    /// a `#[sluggable(...)]` struct.
    fn sluggable() -> bool
    where
        Self: Sized,
    {
        false
    }

    /// The slug-generation configuration (only used when [`Model::sluggable`]).
    fn slug_options() -> crate::sluggable::SlugOptions
    where
        Self: Sized,
    {
        crate::sluggable::SlugOptions::default()
    }

    /// Store a freshly generated slug on the instance (no-op by default).
    fn set_slug(&mut self, slug: &str) {
        let _ = slug;
    }

    /// The current values of the configured slug source columns.
    ///
    /// The derive emits `(column, value)` pairs per `#[sluggable]` source field;
    /// the default returns nothing. Fed to
    /// [`crate::sluggable::SlugOptions::generate`].
    fn slug_source_values(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// Update tracked timestamps in memory (derive macro also emits `touch`).
    fn touch(&mut self) {}

    /// Insert SQL: caller columns, dialect-aware conflict guard.
    fn insert_sql(_table: &str, builder: &InsertBuilder) -> Result<String>
    where
        Self: Sized,
    {
        crate::model_sql::insert_sql(builder)
    }

    /// Upsert SQL with a strict `uniqueBy` (empty → typed error).
    fn upsert_sql(builder: &UpsertBuilder) -> Result<String>
    where
        Self: Sized,
    {
        crate::model_sql::upsert_sql(builder)
    }

    /// Update SQL: `UPDATE t SET <cols> WHERE id = ?` with an optional timestamp bump.
    fn update_sql(table: &str, assignments: &[String]) -> String
    where
        Self: Sized,
    {
        crate::model_sql::update_sql(table, assignments, Self::updated_column())
    }

    /// Delete SQL: hard delete by primary key.
    fn delete_sql(table: &str) -> String {
        crate::model_sql::delete_sql(table)
    }

    /// Soft-delete SQL: `UPDATE t SET deleted_at = now() WHERE id = ?`.
    fn soft_delete_sql(table: &str) -> String {
        crate::model_sql::soft_delete_sql(table)
    }

    /// Restore SQL: `UPDATE t SET deleted_at = NULL WHERE id = ?`.
    fn restore_sql(table: &str) -> String {
        crate::model_sql::restore_sql(table)
    }

    /// Whether query/model result caching is enabled for this model.
    fn cacheable() -> bool {
        false
    }

    /// The default TTL for this model's cached queries (`None` = forever).
    fn cache_ttl() -> Option<std::time::Duration> {
        None
    }

    /// Start a filtered query on the model table with its global scopes.
    ///
    /// [`Model::global_scopes`] are attached and applied at execution, so deleted
    /// rows are hidden by default. A `#[cacheable]` model also gets its default
    /// policy here (the `cache::model_query` helper); a caller may override it
    /// with `.without_cache()` or another `.cache(...)`.
    fn query() -> QueryBuilder
    where
        Self: Sized,
    {
        crate::cache::model_query(
            QueryBuilder::table(Self::table_name())
                .with_global_scopes(Self::global_scopes())
                .with_relations(Self::relations()),
            Self::cacheable(),
            Self::cache_ttl(),
        )
    }

    /// Query that includes soft-deleted rows.
    ///
    /// Delegates to [`Model::query`] (preserving relations metadata) then
    /// bypasses the built-in [`SoftDeletesScope`]. The cache policy is cleared:
    /// the write path re-reads a row through this query right after mutating it
    /// (before the generation bump), so it must read through to the database.
    fn query_with_trashed() -> QueryBuilder
    where
        Self: Sized,
    {
        Self::query()
            .without_global_scope::<SoftDeletesScope>()
            .without_cache()
    }

    /// Query restricted to soft-deleted rows.
    ///
    /// Delegates to [`Model::query`] (preserving relations metadata) and keeps
    /// only rows where the soft-delete marker is set. Like
    /// [`Model::query_with_trashed`], the cache policy is cleared.
    fn query_only_trashed() -> QueryBuilder
    where
        Self: Sized,
    {
        Self::query()
            .without_global_scope::<SoftDeletesScope>()
            .only_trashed()
            .without_cache()
    }

    /// Raw SELECT fragment over the model table (`Model::raw_sql`).
    fn raw_select(sql: &str) -> Result<Raw>
    where
        Self: Sized,
    {
        crate::model_sql::raw_select(&Self::table_name(), sql)
    }

    /// Build a SELECT that refreshes the row for update (SQLite rejects `FOR UPDATE`).
    fn refresh_for_update(id: Uuid, dialect: &str) -> Result<QueryBuilder>
    where
        Self: Sized,
    {
        let base =
            QueryBuilder::table(Self::table_name()).with_global_scopes(Self::global_scopes());
        base.where_eq("id", Value::Uuid(id))
            .for_update_with_dialect(dialect)
    }

    /// COUNT projection honoring the current filters.
    fn count_query(builder: &QueryBuilder) -> Result<String>
    where
        Self: Sized,
    {
        count_sql(builder)
    }

    /// Chunk the model table by primary-key windows.
    fn chunk_by(size: u64, total: u64) -> Result<Vec<String>>
    where
        Self: Sized,
    {
        crate::execution::chunk_by(&Self::query(), "id", size, total)
    }

    /// `INSERT OR IGNORE` returning the inserted ids (dialect-aware).
    fn insert_or_ignore_returning(table: &str, builder: &InsertBuilder) -> Result<String>
    where
        Self: Sized,
    {
        Self::insert_sql(table, builder)
    }

    /// Resolve the row for update and surface typed errors on missing rows.
    ///
    /// `dialect` is threaded into [`Model::refresh_for_update`] so the lock
    /// decision tracks the live driver.
    fn first_for_update(id: Uuid, dialect: &str) -> Result<QueryBuilder>
    where
        Self: Sized,
    {
        let qb = Self::refresh_for_update(id, dialect)?;
        if qb.has_limit() {
            return Err(OrmError::InvalidState(
                "first_for_update cannot carry a LIMIT".into(),
            ));
        }
        Ok(qb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies belongs_to derives a `snake_singular` FK across plural rules.
    #[test]
    fn belongs_to_singularizes_parent_table() {
        let r = Relation::belongs_to("category", "categories");
        assert_eq!(r.foreign_key, "category_id");
        let r = Relation::belongs_to("post", "posts");
        assert_eq!(r.foreign_key, "post_id");
    }
}
