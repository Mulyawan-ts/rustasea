//! Unit tests for the `show:model` source-level inspector.
//!
//! Split from `model_inspect.rs` to keep that module within the 500-line
//! standard, mirroring `rustasea-orm/src/relation.rs` → `relation/tests.rs`.

use super::*;

/// A derived model with casts, tracked columns and two relations.
const USER_SRC: &str = r#"
use rustasea_macros::Model;

#[derive(Model)]
struct User {
    id: Uuid,
    name: String,
    #[model(cast = "json")]
    settings: Settings,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

impl Model for User {
    fn relations() -> Vec<Relation> {
        vec![
            Relation::has_many("posts", "posts", "User"),
            Relation::belongs_to("team", "teams"),
        ]
    }
}
"#;

/// Verifies the full attribute/cast/relation/flags extraction.
#[test]
fn inspects_full_model() {
    let inspection = inspect_source(USER_SRC, "User").expect("parses");
    assert_eq!(inspection.model, "User");
    assert_eq!(inspection.table, "users");
    assert!(inspection.soft_delete);
    assert!(inspection.timestamps);
    assert_eq!(inspection.attributes.len(), 6);
    assert_eq!(inspection.attributes[0].name, "id");
    assert_eq!(inspection.attributes[0].type_name, "Uuid");
    assert!(!inspection.attributes[0].nullable);
    let deleted = inspection
        .attributes
        .iter()
        .find(|a| a.name == "deleted_at")
        .expect("deleted_at present");
    assert!(deleted.nullable);
    assert_eq!(deleted.type_name, "Option<DateTime<Utc>>");
    assert_eq!(inspection.casts.get("settings"), Some(&"json".to_string()));
    assert_eq!(inspection.relations.len(), 2);
    assert_eq!(inspection.relations[0].name, "posts");
    assert_eq!(inspection.relations[0].kind, "HasMany");
    assert_eq!(inspection.relations[1].kind, "BelongsTo");
}

/// Verifies an explicit table override and `soft_deletes = "none"`.
#[test]
fn honors_table_override_and_soft_delete_off() {
    let src = r#"
#[derive(Model)]
#[model(table = "people", soft_deletes = "none")]
struct Person {
    id: Uuid,
    name: String,
    deleted_at: Option<DateTime<Utc>>,
}
"#;
    let inspection = inspect_source(src, "Person").expect("parses");
    assert_eq!(inspection.table, "people");
    assert!(!inspection.soft_delete);
    // No created_at/updated_at fields, so timestamps stays false.
    assert!(!inspection.timestamps);
}

/// Verifies the canonical `snake_plural` table derivation.
#[test]
fn derives_canonical_tables() {
    let src = "#[derive(Model)]\nstruct Category { id: Uuid }\n";
    let inspection = inspect_source(src, "Category").expect("parses");
    assert_eq!(inspection.table, "categories");

    let src = "#[derive(Model)]\nstruct Post { id: Uuid }\n";
    assert_eq!(inspect_source(src, "Post").expect("parses").table, "posts");
}

/// Verifies `_json` and `many_to_many` constructors map onto contract kinds.
#[test]
fn maps_relation_kinds() {
    let src = r#"
#[derive(Model)]
struct Node { id: Uuid }

impl Model for Node {
    fn relations() -> Vec<Relation> {
        vec![
            Relation::has_many_json("tags", "tags", "payload", "tag_ids").unwrap(),
            Relation::belongs_to_json("author", "users", "payload", "author.id").unwrap(),
            Relation::many_to_many("roles", "roles", "role_user", "Node", "Role"),
            Relation::belongs_to_many_json("labels", "labels", "payload", "label_ids").unwrap(),
        ]
    }
}
"#;
    let inspection = inspect_source(src, "Node").expect("parses");
    let kinds: Vec<&str> = inspection
        .relations
        .iter()
        .map(|r| r.kind.as_str())
        .collect();
    assert_eq!(
        kinds,
        vec!["HasMany", "BelongsTo", "BelongsToMany", "BelongsToMany"]
    );
    assert_eq!(inspection.relations[0].name, "tags");
}

/// Verifies the derive marker wins over an earlier non-model struct.
#[test]
fn selects_derived_struct() {
    let src = r#"
struct Helper { value: i64 }

#[derive(Model)]
struct Widget { id: Uuid }
"#;
    let inspection = inspect_source(src, "Widget").expect("parses");
    assert_eq!(inspection.model, "Widget");
}

/// Verifies the hand-written `impl Model` scaffold style is introspected.
///
/// Mirrors the `make:model` generator output
/// (`crates/rustasea-cli/src/generators/kinds/model.rs`) and the app scaffold
/// (`crates/rustasea-scaffold/src/templates/app_domain.rs`), which declare the
/// table, soft-delete and timestamp flags by hand instead of `#[derive(Model)]`.
#[test]
fn inspects_hand_written_impl_model() {
    let src = r#"
use rustasea::orm::{Model, Timestamps, SoftDeletes};

pub struct Article {
    pub id: Uuid,
    pub title: String,
    pub published_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub timestamps: Timestamps,
}

impl Model for Article {
    fn type_name() -> &'static str {
        "Article"
    }

    fn table_name() -> String {
        "articles".to_string()
    }

    fn primary_key(&self) -> Uuid {
        self.id
    }

    fn assign_id(&mut self) -> Uuid {
        self.id = Uuid::now_v7();
        self.id
    }

    fn uses_soft_deletes() -> bool {
        true
    }

    fn uses_timestamps() -> bool {
        true
    }

    fn relations() -> Vec<Relation> {
        vec![Relation::has_many("comments", "comments", "Article")]
    }
}
"#;
    let inspection = inspect_source(src, "Article").expect("parses");
    assert_eq!(inspection.model, "Article");
    assert_eq!(inspection.table, "articles");
    assert!(inspection.soft_delete);
    assert!(inspection.timestamps);
    assert!(inspection
        .attributes
        .iter()
        .any(|a| a.name == "published_at" && a.nullable));
    assert_eq!(inspection.relations[0].kind, "HasMany");
}

/// Verifies the scaffold `table_name()` override wins over `snake_plural`.
#[test]
fn hand_written_table_override_wins() {
    let src = r#"
pub struct Person { pub id: Uuid }

impl Model for Person {
    fn type_name() -> &'static str { "Person" }
    fn table_name() -> String { "people".to_string() }
    fn primary_key(&self) -> Uuid { self.id }
    fn assign_id(&mut self) -> Uuid { self.id }
}
"#;
    let inspection = inspect_source(src, "Person").expect("parses");
    assert_eq!(inspection.table, "people");
    // No explicit flags and no tracked columns or marker fields: both stay off.
    assert!(!inspection.soft_delete);
    assert!(!inspection.timestamps);
}

/// Verifies the `make:model` scaffold shape (marker fields, no explicit flags).
#[test]
fn inspects_make_model_scaffold() {
    let src = r#"
pub struct Comment {
    pub id: uuid::Uuid,
    pub timestamps: Timestamps,
    pub soft_deletes: SoftDeletes,
}

impl Model for Comment {
    fn type_name() -> &'static str { "Comment" }
    fn primary_key(&self) -> uuid::Uuid { self.id }
    fn assign_id(&mut self) -> uuid::Uuid { self.id }
}
"#;
    let inspection = inspect_source(src, "Comment").expect("parses");
    assert_eq!(inspection.table, "comments");
    // Marker fields stand in for the tracked columns in scaffolded models.
    assert!(inspection.soft_delete);
    assert!(inspection.timestamps);
}

/// Verifies a source with no struct reports `StructNotFound`.
#[test]
fn reports_missing_struct() {
    let error = inspect_source("fn main() {}", "Ghost").expect_err("no struct");
    assert!(matches!(error, ModelInspectError::StructNotFound { .. }));
    assert!(error.to_string().contains("Ghost"));
}

/// Verifies malformed source reports a parse error rather than panicking.
#[test]
fn reports_malformed_source() {
    let error = inspect_source("struct {", "Broken").expect_err("invalid");
    assert!(matches!(error, ModelInspectError::Parse { .. }));
}

/// Verifies `inspect_model` reads the expected path and reports misses.
#[test]
fn inspect_model_reads_and_reports_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let models = dir.path().join("app/models");
    std::fs::create_dir_all(&models).expect("mkdir");
    std::fs::write(models.join("user.rs"), USER_SRC).expect("write fixture");

    let inspection = inspect_model(dir.path(), "User").expect("reads model");
    assert_eq!(inspection.table, "users");

    let error = inspect_model(dir.path(), "Ghost").expect_err("missing");
    assert!(matches!(error, ModelInspectError::FileNotFound { .. }));
    assert!(error.to_string().contains("app/models/ghost.rs"));
}
