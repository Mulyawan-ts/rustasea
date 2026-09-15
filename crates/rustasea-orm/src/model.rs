//! Model trait, timestamps, soft deletes, and relations.

use crate::builder::{QueryBuilder, Raw, SqlFragment};
use crate::casts::CastBinding;
use crate::error::{OrmError, Result};
use crate::execution::count_sql;
use crate::m2::{InsertBuilder, ModelScopes, UpsertBuilder};
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

/// Kind of relation between two models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    /// `User has_many posts` — FK `user_id` on the related table.
    HasMany,
    /// Inverse of HasMany — FK on this table.
    BelongsTo,
    /// Pivot-table relation (scaffolded; full impl M3+).
    ManyToMany,
}

/// A declared relation used by eager loading (`with`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    /// Relation name as used in `Model::with("posts")`.
    pub name: String,
    /// Related model's table name.
    pub related_table: String,
    /// FK column on the owning side (HasMany: related table; BelongsTo: this table).
    pub foreign_key: String,
    /// Local key on the parent side.
    pub local_key: String,
    /// Relation kind.
    pub kind: RelationKind,
    /// Pivot table for `ManyToMany` relations (`None` otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pivot_table: Option<String>,
    /// Related-side pivot column for `ManyToMany` relations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related_key: Option<String>,
}

impl Relation {
    /// Declare a HasMany relation using the `snake_singular` FK convention.
    pub fn has_many(name: &str, related_table: &str, local_model: &str) -> Self {
        let fk = format!("{}_id", crate::naming::to_snake_case(local_model));
        Self {
            name: name.to_string(),
            related_table: related_table.to_string(),
            foreign_key: fk,
            local_key: "id".to_string(),
            kind: RelationKind::HasMany,
            pivot_table: None,
            related_key: None,
        }
    }

    /// Declare a BelongsTo relation using the `snake_singular` FK convention.
    pub fn belongs_to(name: &str, parent_table: &str) -> Self {
        let fk = format!("{}_id", crate::naming::singular_from_plural(parent_table));
        Self {
            name: name.to_string(),
            related_table: parent_table.to_string(),
            foreign_key: fk,
            local_key: "id".to_string(),
            kind: RelationKind::BelongsTo,
            pivot_table: None,
            related_key: None,
        }
    }

    /// Declare a ManyToMany relation through `pivot_table`.
    ///
    /// The pivot's parent column is `{snake(local_model)}_id` and its related
    /// column is `{snake(related_model)}_id`, matching the Eloquent convention.
    pub fn many_to_many(
        name: &str,
        related_table: &str,
        pivot_table: &str,
        local_model: &str,
        related_model: &str,
    ) -> Self {
        let foreign_key = format!("{}_id", crate::naming::to_snake_case(local_model));
        let related_key = format!("{}_id", crate::naming::to_snake_case(related_model));
        Self {
            name: name.to_string(),
            related_table: related_table.to_string(),
            foreign_key,
            local_key: "id".to_string(),
            kind: RelationKind::ManyToMany,
            pivot_table: Some(pivot_table.to_string()),
            related_key: Some(related_key),
        }
    }
}

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

    /// Declared relations for eager loading.
    fn relations() -> Vec<Relation>
    where
        Self: Sized,
    {
        Vec::new()
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
        ModelScopes::insert_sql(builder, crate::builder::dialect())
    }

    /// Upsert SQL with a strict `uniqueBy` (empty → typed error).
    fn upsert_sql(builder: &UpsertBuilder) -> Result<String>
    where
        Self: Sized,
    {
        builder.to_sql()
    }

    /// Update SQL: `UPDATE t SET <cols> WHERE id = ?`.
    ///
    /// `assignments` are caller-owned `col = ?`-shaped fragments; the primary
    /// key filter and optional `updated_at = now()` bump are appended.
    fn update_sql(table: &str, assignments: &[String]) -> String
    where
        Self: Sized,
    {
        let mut parts = assignments.to_vec();
        if let Some(col) = Self::updated_column() {
            parts.push(format!("{col} = now()"));
        }
        let cols = parts.join(", ");
        format!("UPDATE {table} SET {cols} WHERE id = $1")
    }

    /// Delete SQL: hard delete by primary key.
    fn delete_sql(table: &str) -> String {
        format!("DELETE FROM {table} WHERE id = $1")
    }

    /// Soft-delete SQL: `UPDATE t SET deleted_at = now() WHERE id = ?`.
    fn soft_delete_sql(table: &str) -> String {
        format!("UPDATE {table} SET deleted_at = now() WHERE id = $1")
    }

    /// Restore SQL: `UPDATE t SET deleted_at = NULL WHERE id = ?`.
    fn restore_sql(table: &str) -> String {
        format!("UPDATE {table} SET deleted_at = NULL WHERE id = $1")
    }

    /// Start a filtered query on the model table with its global scopes.
    ///
    /// The model's [`Model::global_scopes`] (soft deletes plus any registered
    /// application scopes) are attached and applied when the query executes, so
    /// deleted rows are hidden by default. Bypass with
    /// [`QueryBuilder::without_global_scopes`].
    fn query() -> QueryBuilder
    where
        Self: Sized,
    {
        QueryBuilder::table(Self::table_name())
            .with_global_scopes(Self::global_scopes())
            .with_relations(Self::relations())
    }

    /// Query that includes soft-deleted rows.
    ///
    /// Delegates to [`Model::query`] so the model's declared relations metadata
    /// ([`Model::relations`]) is preserved for eager loading, then bypasses the
    /// built-in [`SoftDeletesScope`].
    fn query_with_trashed() -> QueryBuilder
    where
        Self: Sized,
    {
        Self::query().without_global_scope::<SoftDeletesScope>()
    }

    /// Query restricted to soft-deleted rows.
    ///
    /// Delegates to [`Model::query`] (preserving relations metadata) and keeps
    /// only rows where the soft-delete marker is set.
    fn query_only_trashed() -> QueryBuilder
    where
        Self: Sized,
    {
        Self::query()
            .without_global_scope::<SoftDeletesScope>()
            .only_trashed()
    }

    /// Raw SELECT fragment over the model table (`Model::raw_sql`).
    fn raw_select(sql: &str) -> Result<Raw>
    where
        Self: Sized,
    {
        let fragment: SqlFragment = sql.into();
        let clause = fragment.sql;
        let full = format!(
            "SELECT * FROM {} WHERE {}",
            Self::table_name(),
            clause.trim()
        );
        Ok(Raw {
            sql: full,
            bindings: Vec::new(),
        })
    }

    /// Build a SELECT that refreshes the row for update.
    ///
    /// `dialect` is the runtime pool dialect; the lock clause is validated and
    /// emitted for that dialect, so an SQLite pool rejects `FOR UPDATE` with
    /// [`OrmError::UnsupportedDriver`] even when `postgres` is compiled in.
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
    /// `dialect` is the runtime pool dialect threaded into
    /// [`Model::refresh_for_update`] so the lock decision tracks the live
    /// driver rather than the compiled feature set.
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
