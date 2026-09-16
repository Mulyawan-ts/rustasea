//! Relation declarations — single-key, composite-key, and JSON-column kinds.
//!
//! The [`Relation`] and [`RelationKind`] types live here and are re-exported
//! from [`crate::model`], so existing `rustasea_orm::model::Relation` paths keep
//! working; this module owns the builder constructors and their validation so
//! `model.rs` stays within the file-size standard.
//!
//! Three families are declared:
//!
//! * Single-key constructors ([`Relation::has_many`], [`Relation::belongs_to`],
//!   [`Relation::many_to_many`]) leave the multi-column fields unset, so a
//!   relation declared the old way is byte-for-byte unchanged.
//! * Composite constructors (awobaz/compoships parity) accept plural key slices
//!   and reject an empty or length-mismatched declaration with a typed
//!   [`OrmError::InvalidState`] *before* any query is built.
//! * JSON constructors (`*_json`, staudenmeir/eloquent-json-relations parity)
//!   carry a [`JsonSpec`] naming the JSON column and path holding the related
//!   keys, matched against the related table's `id` column.

use crate::error::{OrmError, Result};
use serde::{Deserialize, Serialize};

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
    /// `Post belongs_to_json user` — the parent's key sits in this row's JSON.
    BelongsToJson,
    /// `User has_many_json posts` — an array of related ids in this row's JSON.
    HasManyJson,
    /// Pivot-less many-to-many — an array of related ids in this row's JSON.
    BelongsToManyJson,
}

/// JSON column + path pair identifying where a `*_json` relation stores keys.
///
/// `column` is the JSON column on the owning table and `path` is the dot-path
/// into that document (e.g. `column = "payload"`, `path = "author.id"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonSpec {
    /// JSON column holding the embedded key(s).
    pub column: String,
    /// Dot-path into the JSON document (e.g. `author.id`).
    pub path: String,
}

impl JsonSpec {
    /// Validate a JSON relation declaration, rejecting empty name/column/path.
    fn validated(builder: &str, name: &str, column: &str, path: &str) -> Result<Self> {
        if name.trim().is_empty() {
            return Err(OrmError::InvalidState(format!(
                "`{builder}` requires a non-empty relation name"
            )));
        }
        if column.trim().is_empty() {
            return Err(OrmError::InvalidState(format!(
                "`{builder}` requires a non-empty JSON column"
            )));
        }
        if path.trim().is_empty() {
            return Err(OrmError::InvalidState(format!(
                "`{builder}` requires a non-empty JSON path"
            )));
        }
        Ok(Self {
            column: column.to_string(),
            path: path.to_string(),
        })
    }
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
    /// Composite foreign-key columns (`Some` only for composite relations).
    ///
    /// `None` for single-key relations, where [`Relation::foreign_key`] holds
    /// the sole column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_keys: Option<Vec<String>>,
    /// Composite local-key columns (`Some` only for composite relations).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_keys: Option<Vec<String>>,
    /// JSON column/path spec (`Some` only for `*_json` relations).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json: Option<JsonSpec>,
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
            foreign_keys: None,
            local_keys: None,
            json: None,
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
            foreign_keys: None,
            local_keys: None,
            json: None,
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
            foreign_keys: None,
            local_keys: None,
            json: None,
        }
    }

    /// Declare a `BelongsToJson` relation (staudenmeir parity).
    ///
    /// The local row's JSON `json_column` at `json_path` holds the parent's key
    /// as a scalar string or number. The parent is matched on its `id` column
    /// and the local key is `id` semantics; the embedded key is resolved against
    /// the related table's `id` column.
    pub fn belongs_to_json(
        name: &str,
        parent_table: &str,
        json_column: &str,
        json_path: &str,
    ) -> Result<Self> {
        let spec = JsonSpec::validated("belongs_to_json", name, json_column, json_path)?;
        Ok(Self::json_relation(
            name,
            parent_table,
            RelationKind::BelongsToJson,
            spec,
        ))
    }

    /// Declare a `HasManyJson` relation (staudenmeir parity).
    ///
    /// The local row's JSON `json_column` at `json_path` holds an **array** of
    /// related ids; the related table is queried by its `id` column and each
    /// parent receives the matching rows.
    pub fn has_many_json(
        name: &str,
        related_table: &str,
        json_column: &str,
        json_path: &str,
    ) -> Result<Self> {
        let spec = JsonSpec::validated("has_many_json", name, json_column, json_path)?;
        Ok(Self::json_relation(
            name,
            related_table,
            RelationKind::HasManyJson,
            spec,
        ))
    }

    /// Declare a `BelongsToManyJson` relation (staudenmeir parity).
    ///
    /// Like [`Relation::has_many_json`], the local row's JSON holds an array of
    /// related ids, but the relation is a pivot-less many-to-many: the ids are
    /// resolved against the related table's `id` column in one batched query.
    pub fn belongs_to_many_json(
        name: &str,
        related_table: &str,
        json_column: &str,
        json_path: &str,
    ) -> Result<Self> {
        let spec = JsonSpec::validated("belongs_to_many_json", name, json_column, json_path)?;
        Ok(Self::json_relation(
            name,
            related_table,
            RelationKind::BelongsToManyJson,
            spec,
        ))
    }

    /// Build a JSON-backed relation with `id` key semantics on both sides.
    fn json_relation(name: &str, related_table: &str, kind: RelationKind, spec: JsonSpec) -> Self {
        Self {
            name: name.to_string(),
            related_table: related_table.to_string(),
            foreign_key: "id".to_string(),
            local_key: "id".to_string(),
            kind,
            pivot_table: None,
            related_key: None,
            foreign_keys: None,
            local_keys: None,
            json: Some(spec),
        }
    }

    /// Declare a composite-key `HasMany` relation.
    ///
    /// `foreign_keys` are the related table's columns; `local_keys` are this
    /// model's columns. Both slices must be non-empty and the same length, else
    /// a typed [`OrmError::InvalidState`] is returned. The first pair is mirrored
    /// onto the singular `foreign_key`/`local_key` fields so single-column
    /// helpers stay usable.
    pub fn has_many_composite(
        name: &str,
        related_table: &str,
        foreign_keys: &[&str],
        local_keys: &[&str],
    ) -> Result<Self> {
        validate_keys("has_many_composite", foreign_keys, local_keys)?;
        Ok(Self::composite(
            name,
            related_table,
            RelationKind::HasMany,
            foreign_keys,
            local_keys,
            None,
            None,
        ))
    }

    /// Declare a composite-key `BelongsTo` relation.
    ///
    /// `foreign_keys` are this model's columns; `local_keys` are the parent
    /// table's columns. Arity validation matches
    /// [`Relation::has_many_composite`].
    pub fn belongs_to_composite(
        name: &str,
        parent_table: &str,
        foreign_keys: &[&str],
        local_keys: &[&str],
    ) -> Result<Self> {
        validate_keys("belongs_to_composite", foreign_keys, local_keys)?;
        Ok(Self::composite(
            name,
            parent_table,
            RelationKind::BelongsTo,
            foreign_keys,
            local_keys,
            None,
            None,
        ))
    }

    /// Declare a composite-key `ManyToMany` relation through `pivot_table`.
    ///
    /// The pivot's parent link is composite (`foreign_keys` on the pivot,
    /// matched against `local_keys` on this model); the related side stays a
    /// single `{snake(related_model)}_id` column.
    pub fn many_to_many_composite(
        name: &str,
        related_table: &str,
        pivot_table: &str,
        related_model: &str,
        foreign_keys: &[&str],
        local_keys: &[&str],
    ) -> Result<Self> {
        validate_keys("many_to_many_composite", foreign_keys, local_keys)?;
        let related_key = format!("{}_id", crate::naming::to_snake_case(related_model));
        Ok(Self::composite(
            name,
            related_table,
            RelationKind::ManyToMany,
            foreign_keys,
            local_keys,
            Some(pivot_table.to_string()),
            Some(related_key),
        ))
    }

    /// Build a composite relation, mirroring the first key pair onto the
    /// singular fields.
    fn composite(
        name: &str,
        related_table: &str,
        kind: RelationKind,
        foreign_keys: &[&str],
        local_keys: &[&str],
        pivot_table: Option<String>,
        related_key: Option<String>,
    ) -> Self {
        Self {
            name: name.to_string(),
            related_table: related_table.to_string(),
            // Validation guarantees a non-empty slice, so indexing is safe.
            foreign_key: foreign_keys[0].to_string(),
            local_key: local_keys[0].to_string(),
            kind,
            pivot_table,
            related_key,
            foreign_keys: Some(foreign_keys.iter().map(|key| (*key).to_string()).collect()),
            local_keys: Some(local_keys.iter().map(|key| (*key).to_string()).collect()),
            json: None,
        }
    }

    /// The relation's foreign-key columns (composite or the single fallback).
    pub fn effective_foreign_keys(&self) -> &[String] {
        match &self.foreign_keys {
            Some(keys) => keys,
            None => std::slice::from_ref(&self.foreign_key),
        }
    }

    /// The relation's local-key columns (composite or the single fallback).
    pub fn effective_local_keys(&self) -> &[String] {
        match &self.local_keys {
            Some(keys) => keys,
            None => std::slice::from_ref(&self.local_key),
        }
    }

    /// Whether this relation keys on more than one column.
    ///
    /// A `*_composite` constructor with a single pair reports `false`, so the
    /// eager loader takes the (equivalent) single-column path.
    pub fn is_composite(&self) -> bool {
        self.foreign_keys
            .as_ref()
            .is_some_and(|keys| keys.len() > 1)
    }

    /// The JSON column/path spec for a `*_json` relation, if any.
    pub fn json_spec(&self) -> Option<&JsonSpec> {
        self.json.as_ref()
    }
}

/// Validate that a composite declaration has matching, non-empty key arity.
///
/// Returns a typed [`OrmError::InvalidState`] so a mis-declared relation fails
/// at build time instead of emitting an unbalanced `IN` predicate.
fn validate_keys(builder: &str, foreign_keys: &[&str], local_keys: &[&str]) -> Result<()> {
    if foreign_keys.is_empty() || local_keys.is_empty() {
        return Err(OrmError::InvalidState(format!(
            "`{builder}` requires at least one key column"
        )));
    }
    if foreign_keys.len() != local_keys.len() {
        return Err(OrmError::InvalidState(format!(
            "`{builder}` key arity mismatch: {} foreign keys vs {} local keys",
            foreign_keys.len(),
            local_keys.len()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
