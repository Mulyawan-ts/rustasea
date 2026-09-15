//! Relation constructors — single- and composite-key declarations.
//!
//! The [`Relation`] type itself lives in [`crate::model`], next to
//! [`crate::model::Model`]; this module owns the builder constructors and their
//! arity validation so `model.rs` stays within the file-size standard. The
//! composite constructors (awobaz/compoships parity) accept plural key slices
//! and reject an empty or length-mismatched declaration with a typed
//! [`OrmError::InvalidState`] *before* any query is built.
//!
//! Single-key constructors ([`Relation::has_many`], [`Relation::belongs_to`],
//! [`Relation::many_to_many`]) leave the multi-column fields unset, so a
//! relation declared the old way is byte-for-byte unchanged.

use crate::error::{OrmError, Result};
use crate::model::{Relation, RelationKind};

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
mod tests {
    use super::*;

    /// Verifies composite constructors capture both key slices and mirror the
    /// first pair onto the singular fields.
    #[test]
    fn composite_constructors_capture_keys() {
        let relation = Relation::has_many_composite(
            "memberships",
            "memberships",
            &["tenant_id", "user_id"],
            &["tenant_id", "id"],
        )
        .expect("valid composite declaration");
        assert!(relation.is_composite());
        assert_eq!(relation.foreign_key, "tenant_id");
        assert_eq!(relation.local_key, "tenant_id");
        assert_eq!(
            relation.effective_foreign_keys(),
            ["tenant_id".to_string(), "user_id".to_string()]
        );
        assert_eq!(
            relation.effective_local_keys(),
            ["tenant_id".to_string(), "id".to_string()]
        );
    }

    /// Verifies a mismatched key arity is rejected with a typed error.
    #[test]
    fn mismatched_arity_is_typed_error() {
        let error =
            Relation::belongs_to_composite("tenant", "tenants", &["tenant_id", "region"], &["id"])
                .expect_err("arity mismatch must fail");
        assert!(matches!(error, OrmError::InvalidState(_)), "got {error:?}");
    }

    /// Verifies an empty key slice is rejected.
    #[test]
    fn empty_keys_are_typed_error() {
        assert!(matches!(
            Relation::has_many_composite("x", "xs", &[], &["id"]),
            Err(OrmError::InvalidState(_))
        ));
        assert!(matches!(
            Relation::many_to_many_composite("x", "xs", "pivot", "X", &["a"], &[]),
            Err(OrmError::InvalidState(_))
        ));
    }

    /// Verifies single-key relations stay non-composite with a one-element
    /// effective key slice.
    #[test]
    fn single_key_relations_stay_single() {
        let relation = Relation::has_many("posts", "posts", "User");
        assert!(!relation.is_composite());
        assert_eq!(relation.effective_foreign_keys(), ["user_id".to_string()]);
        assert_eq!(relation.effective_local_keys(), ["id".to_string()]);
    }
}
