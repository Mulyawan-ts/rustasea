//! Unit tests for the relation constructors (composite + JSON).

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
    assert!(relation.json_spec().is_none());
}

/// Verifies the three JSON constructors capture their column/path spec and
/// report the matching relation kind.
#[test]
fn json_constructors_capture_spec() {
    let belongs_to = Relation::belongs_to_json("author", "users", "payload", "author.id")
        .expect("valid belongs_to_json");
    assert_eq!(belongs_to.kind, RelationKind::BelongsToJson);
    assert_eq!(
        belongs_to.json_spec(),
        Some(&JsonSpec {
            column: "payload".into(),
            path: "author.id".into(),
        })
    );
    assert_eq!(belongs_to.foreign_key, "id");
    assert_eq!(belongs_to.local_key, "id");

    let has_many =
        Relation::has_many_json("tags", "tags", "payload", "tag_ids").expect("valid has_many_json");
    assert_eq!(has_many.kind, RelationKind::HasManyJson);

    let belongs_to_many =
        Relation::belongs_to_many_json("labels", "labels", "payload", "label_ids")
            .expect("valid belongs_to_many_json");
    assert_eq!(belongs_to_many.kind, RelationKind::BelongsToManyJson);
}

/// Verifies an empty column or path is rejected with a typed error.
#[test]
fn json_constructors_reject_empty_parts() {
    assert!(matches!(
        Relation::belongs_to_json("author", "users", "", "author.id"),
        Err(OrmError::InvalidState(_))
    ));
    assert!(matches!(
        Relation::has_many_json("tags", "tags", "payload", ""),
        Err(OrmError::InvalidState(_))
    ));
    assert!(matches!(
        Relation::belongs_to_many_json("", "labels", "payload", "label_ids"),
        Err(OrmError::InvalidState(_))
    ));
}
