//! `#[derive(Model)]` compile smoke tests (M2).
//!
//! The derive lives in `rustasea-macros`; these integration tests exercise it
//! against real structs through the public `rustasea_orm` surface so the
//! emitted `Model` impl (table naming, timestamps, soft deletes, touch)
//! compiles and behaves end to end.

use chrono::{DateTime, Utc};
use rustasea_orm::{CastsAttributes, Model, SoftDeletes, Timestamps, Value};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Canonical derived model — snake_plural `users`, tracked timestamps and
/// soft deletes inferred from the field layout.
#[allow(dead_code)]
#[derive(rustasea_macros::Model)]
struct User {
    id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

/// Derived model with an explicit table override and soft deletes disabled.
#[allow(dead_code)]
#[derive(rustasea_macros::Model)]
#[model(table = "people", soft_deletes = "none")]
struct Person {
    id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Verifies table_name derives snake_plural from the type name.
#[test]
fn derive_names_table_via_snake_plural() {
    assert_eq!(<User as Model>::table_name(), "users");
    assert_eq!(<User as Model>::type_name(), "User");
}

/// Verifies an explicit `#[model(table = "...")]` overrides the derivation.
#[test]
fn derive_honors_explicit_table() {
    assert_eq!(<Person as Model>::table_name(), "people");
}

/// Verifies timestamps/soft-delete flags are inferred from fields.
#[test]
fn derive_infers_tracked_columns() {
    assert!(<User as Model>::uses_timestamps());
    assert!(<User as Model>::uses_soft_deletes());
    assert_eq!(
        <User as Model>::insert_columns(),
        vec!["created_at", "updated_at"]
    );
    assert_eq!(<User as Model>::updated_column(), Some("updated_at"));
    assert_eq!(<User as Model>::deleted_column(), Some("deleted_at"));

    // `soft_deletes = "none"` — and no `deleted_at` field either.
    assert!(!<Person as Model>::uses_soft_deletes());
    assert_eq!(<Person as Model>::deleted_column(), None);
}

/// Verifies the emitted primary-key and assign_id helpers work on an instance.
#[test]
fn derive_primary_key_and_assign_id() {
    let mut user = User {
        id: Uuid::now_v7(),
        name: "Ada".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
    };
    assert_eq!(user.primary_key(), user.id);
    let fresh = user.assign_id();
    assert_eq!(fresh, user.id);
    assert_ne!(fresh, Uuid::nil());
}

/// Verifies the default query gate carries the soft-delete guard and trashed
/// queries remove it — all through the Model query helpers.
#[test]
fn derive_query_helpers_emit_soft_delete_guards() {
    let active = <User as Model>::query();
    assert!(active
        .to_sql()
        .unwrap()
        .ends_with("WHERE deleted_at IS NULL"));

    let trashed = <User as Model>::query_with_trashed();
    assert_eq!(trashed.to_sql().unwrap(), "SELECT * FROM users");

    let only = <User as Model>::query_only_trashed();
    assert!(only
        .to_sql()
        .unwrap()
        .ends_with("WHERE deleted_at IS NOT NULL"));
}

/// Hand-written model declaring a relation, used to prove the trashed query
/// helpers preserve relation metadata.
struct Author {
    id: Uuid,
}

impl Model for Author {
    fn type_name() -> &'static str {
        "Author"
    }

    fn primary_key(&self) -> Uuid {
        self.id
    }

    fn assign_id(&mut self) -> Uuid {
        self.id = Uuid::now_v7();
        self.id
    }

    fn relations() -> Vec<rustasea_orm::Relation> {
        vec![rustasea_orm::Relation::has_many("posts", "posts", "Author")]
    }
}

/// Verifies the trashed query helpers preserve the model's declared relations
/// metadata, so `with(&[...]).get_eager()` can resolve relation names.
#[test]
fn derive_trashed_queries_preserve_relations() {
    use rustasea_orm::SoftDeletesScope;

    let expected = Author::relations();

    let trashed = <Author as Model>::query_with_trashed();
    assert!(!trashed.has_global_scope::<SoftDeletesScope>());
    assert_eq!(trashed.declared_relations(), expected.as_slice());

    let only = <Author as Model>::query_only_trashed();
    assert!(!only.has_global_scope::<SoftDeletesScope>());
    assert_eq!(only.declared_relations(), expected.as_slice());

    // Sanity: the helpers still emit the expected soft-delete shapes.
    assert_eq!(trashed.to_sql().unwrap(), "SELECT * FROM authors");
    assert!(only
        .to_sql()
        .unwrap()
        .ends_with("WHERE deleted_at IS NOT NULL"));
}

/// Verifies `touch` bumps only the tracked updated_at field.
#[test]
fn derive_touch_bumps_updated_at() {
    let mut user = User {
        id: Uuid::now_v7(),
        name: "Ada".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
    };
    let before = user.updated_at;
    user.touch();
    assert!(user.updated_at >= before);
}

/// Verifies a model with trackable timestamp wrapper types still compiles
/// (wrappers keep the hand-written style used by `make:model` scaffolds).
#[allow(dead_code)]
#[derive(rustasea_macros::Model)]
#[model(soft_deletes = "none", timestamps = "none")]
struct Account {
    id: Uuid,
    name: String,
    timestamps: Timestamps,
    soft_deletes: SoftDeletes,
}

/// A JSON-backed value type exercised by the `json` cast.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Settings {
    theme: String,
}

/// A custom cast proving `cast_with` accepts user-defined structs.
struct UpperCast;

impl CastsAttributes<String> for UpperCast {
    /// Uppercase the stored text.
    fn get(&self, _key: &str, value: &Value) -> rustasea_orm::Result<String> {
        match value {
            Value::Text(text) => Ok(text.to_uppercase()),
            other => Err(rustasea_orm::OrmError::CastError {
                column: "nickname".into(),
                message: format!("expected text, got {other:?}"),
            }),
        }
    }

    /// Persist the text unchanged.
    fn set(&self, _key: &str, value: &String) -> rustasea_orm::Result<Value> {
        Ok(Value::Text(value.clone()))
    }
}

/// Derived model exercising built-in and custom casts, plus a nullable cast.
#[allow(dead_code)]
#[derive(rustasea_macros::Model)]
#[model(soft_deletes = "none", timestamps = "none")]
struct Profile {
    id: Uuid,
    #[model(cast = "json")]
    settings: Settings,
    #[model(cast = "integer")]
    score: i64,
    #[model(cast = "boolean")]
    active: bool,
    #[model(cast = "json")]
    nickname: Option<String>,
    #[model(cast_with = "UpperCast")]
    label: String,
}

/// Verifies the derive records every declared cast on `Model::casts`.
#[test]
fn derive_records_declared_casts() {
    let columns: Vec<&str> = <Profile as Model>::casts()
        .iter()
        .map(|binding| binding.column)
        .collect();
    assert_eq!(
        columns,
        vec!["settings", "score", "active", "nickname", "label"]
    );
}

/// Verifies a JSON cast hydrates a text column into the target struct and a
/// corrupt payload is a typed cast error.
#[test]
fn derive_json_cast_hydrates_and_rejects_corrupt() {
    let casts = <Profile as Model>::casts();
    let settings = casts
        .iter()
        .find(|binding| binding.column == "settings")
        .expect("settings cast is declared");

    let hydrated =
        (settings.get)("settings", Value::Text(r#"{"theme":"dark"}"#.to_string())).unwrap();
    let parsed: Settings = serde_json::from_value(hydrated).unwrap();
    assert_eq!(
        parsed,
        Settings {
            theme: "dark".into()
        }
    );

    let error = (settings.get)("settings", Value::Text("not-json".into()))
        .expect_err("corrupt JSON must fail");
    assert!(
        matches!(error, rustasea_orm::OrmError::CastError { .. }),
        "got {error:?}"
    );
}

/// Verifies the custom `cast_with` cast is wired in both directions.
#[test]
fn derive_custom_cast_round_trips() {
    let casts = <Profile as Model>::casts();
    let label = casts
        .iter()
        .find(|binding| binding.column == "label")
        .expect("label cast is declared");

    let hydrated = (label.get)("label", Value::Text("hi".into())).unwrap();
    assert_eq!(hydrated, serde_json::json!("HI"));

    let bound = (label.set)("label", &serde_json::json!("hi")).unwrap();
    assert_eq!(bound, Value::Text("hi".into()));
}

/// Verifies an optional cast hydrates NULL to a JSON null (→ `None`).
#[test]
fn derive_nullable_cast_handles_null() {
    let casts = <Profile as Model>::casts();
    let nickname = casts
        .iter()
        .find(|binding| binding.column == "nickname")
        .expect("nickname cast is declared");

    let hydrated = (nickname.get)("nickname", Value::Null).unwrap();
    assert!(hydrated.is_null());
    let bound = (nickname.set)("nickname", &serde_json::Value::Null).unwrap();
    assert_eq!(bound, Value::Null);
}

/// Verifies wrapper-style structs without raw datetime columns report no
/// tracked columns, so `uses_*` gates stay false for the manual style.
#[test]
fn derive_wrapper_style_has_no_tracked_columns() {
    assert!(!<Account as Model>::uses_timestamps());
    assert!(!<Account as Model>::uses_soft_deletes());
    assert_eq!(<Account as Model>::insert_columns().len(), 0);
    assert_eq!(<Account as Model>::updated_column(), None);
    assert_eq!(<Account as Model>::deleted_column(), None);
    assert_eq!(<Account as Model>::table_name(), "accounts");

    let account = Account {
        id: Uuid::now_v7(),
        name: "Acme".into(),
        timestamps: Timestamps::default(),
        soft_deletes: SoftDeletes::default(),
    };
    let pk = account.primary_key();
    let ts = account.timestamps.created_at;
    assert_eq!(pk, account.id);
    assert!(!account.name.is_empty());
    assert!(ts <= account.timestamps.updated_at);
    assert!(account.soft_deletes.deleted_at.is_none());
}

/// A model opting into slug generation from its `name` field.
#[allow(dead_code)]
#[derive(rustasea_macros::Model)]
#[model(soft_deletes = "none", timestamps = "none")]
#[sluggable(source = "name")]
struct Post {
    id: Uuid,
    name: String,
    slug: String,
}

/// Verifies `#[sluggable(source = "name")]` emits the slug policy and helpers.
#[test]
fn derive_emits_slug_policy_and_helpers() {
    assert!(<Post as Model>::sluggable());

    let options = <Post as Model>::slug_options();
    assert_eq!(options.source, vec!["name".to_string()]);
    assert_eq!(options.slug_column, "slug");
    assert_eq!(options.separator, '-');
    assert!(options.unique);
    assert!(!options.on_update);
    assert_eq!(options.max_len, 255);

    let mut post = Post {
        id: Uuid::now_v7(),
        name: "Hello World".into(),
        slug: String::new(),
    };
    assert_eq!(
        post.slug_source_values(),
        vec![("name".to_string(), "Hello World".to_string())]
    );
    post.set_slug("hello-world");
    assert_eq!(post.slug, "hello-world");
}

/// Verifies a plain model (no `#[sluggable]`) keeps the opt-out defaults.
#[test]
fn derive_defaults_to_non_sluggable() {
    assert!(!<User as Model>::sluggable());
    assert!(<User as Model>::slug_options().source.is_empty());

    let user = User {
        id: Uuid::now_v7(),
        name: "Ada".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
    };
    assert!(user.slug_source_values().is_empty());
}
