//! Unit tests for the write-path helpers in [`super`].
//!
//! Covers UUID bind fidelity and the automatic timestamp contract: insert fills
//! `created_at`/`updated_at` (preserving any caller-supplied stamp), update
//! refreshes `updated_at` only, a model that opts out writes no stamps, and the
//! bind shape tracks the runtime dialect.

use super::*;
use chrono::{DateTime, Utc};

use super::timestamps::{provided_timestamp, timestamp_value};
use super::write::{build_update, insert_columns_and_bindings, json_to_value};

/// A fixed UUID string used across the type-fidelity assertions.
const UUID_TEXT: &str = "f3c1f3c1-0000-4000-8000-000000000000";

/// Verifies `*_id`/`*_uuid` strings bind as native UUIDs, not text.
#[test]
fn uuid_columns_bind_native_uuid() {
    for column in ["id", "user_id", "owner_uuid"] {
        let value = json_to_value(column, &serde_json::Value::String(UUID_TEXT.into())).unwrap();
        assert_eq!(value, Value::Uuid(Uuid::parse_str(UUID_TEXT).unwrap()));
    }
}

/// Verifies ordinary text columns keep a UUID-looking string as text.
#[test]
fn non_uuid_columns_keep_text() {
    let value = json_to_value("nickname", &serde_json::Value::String(UUID_TEXT.into())).unwrap();
    assert_eq!(value, Value::Text(UUID_TEXT.into()));
}

/// Verifies a non-UUID string in a UUID column is a typed error, not a bind.
#[test]
fn invalid_uuid_column_value_is_typed_error() {
    let error = json_to_value("user_id", &serde_json::Value::String("not-a-uuid".into()))
        .expect_err("must reject non-UUID in a UUID column");
    assert!(matches!(error, OrmError::InvalidValue(_)), "got {error:?}");
}

/// A model with nullable timestamps: `None` means "let the ORM fill it".
#[derive(Serialize)]
struct Stamped {
    id: Uuid,
    name: String,
    created_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
}

impl Model for Stamped {
    fn type_name() -> &'static str {
        "Stamped"
    }
    fn primary_key(&self) -> Uuid {
        self.id
    }
    fn assign_id(&mut self) -> Uuid {
        self.id = Uuid::now_v7();
        self.id
    }
}

/// A model that opts out of timestamps entirely.
#[derive(Serialize)]
struct Plain {
    id: Uuid,
    name: String,
}

impl Model for Plain {
    fn type_name() -> &'static str {
        "Plain"
    }
    fn primary_key(&self) -> Uuid {
        self.id
    }
    fn assign_id(&mut self) -> Uuid {
        self.id = Uuid::now_v7();
        self.id
    }
    fn uses_timestamps() -> bool {
        false
    }
}

/// Extract the bind for `column` from a `(names, bindings)` pair.
fn bind_for<'a>(names: &[String], bindings: &'a [Value], column: &str) -> &'a Value {
    let index = names
        .iter()
        .position(|name| name == column)
        .unwrap_or_else(|| panic!("column `{column}` is missing from {names:?}"));
    &bindings[index]
}

/// A fixed instant used to prove a caller-supplied stamp is preserved.
fn fixed() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2020-01-02T03:04:05Z")
        .unwrap()
        .with_timezone(&Utc)
}

/// Verifies insert fills both timestamps when the model leaves them unset.
#[test]
fn insert_fills_both_timestamps_when_unset() {
    let data = Stamped {
        id: Uuid::now_v7(),
        name: "Ada".into(),
        created_at: None,
        updated_at: None,
    };
    let (names, bindings) = insert_columns_and_bindings(&data, "sqlite").unwrap();

    let created = bind_for(&names, &bindings, "created_at");
    let updated = bind_for(&names, &bindings, "updated_at");
    assert!(
        matches!(created, Value::Text(_)),
        "sqlite stores RFC3339 text"
    );
    assert_eq!(created, updated, "both stamps share the same instant");
}

/// Verifies an explicitly-set timestamp is preserved (Laravel semantics).
#[test]
fn insert_preserves_explicit_timestamps() {
    let stamp = fixed();
    let data = Stamped {
        id: Uuid::now_v7(),
        name: "Ada".into(),
        created_at: Some(stamp),
        updated_at: None,
    };
    let (names, bindings) = insert_columns_and_bindings(&data, "postgres").unwrap();

    assert_eq!(
        bind_for(&names, &bindings, "created_at"),
        &Value::Timestamp(stamp),
        "an explicitly-set created_at must not be overwritten"
    );
    assert!(
        matches!(
            bind_for(&names, &bindings, "updated_at"),
            Value::Timestamp(_)
        ),
        "the unset updated_at is filled with now"
    );
}

/// Verifies a disabled-timestamp model writes no timestamp columns.
#[test]
fn insert_skips_timestamps_when_disabled() {
    let data = Plain {
        id: Uuid::now_v7(),
        name: "Ada".into(),
    };
    let (names, _) = insert_columns_and_bindings(&data, "sqlite").unwrap();
    assert!(!names.iter().any(|name| name.ends_with("_at")));
    assert_eq!(names, vec!["id".to_string(), "name".to_string()]);
}

/// Verifies update refreshes `updated_at` without touching `created_at`.
#[test]
fn update_refreshes_only_updated_at() {
    let data = Stamped {
        id: Uuid::now_v7(),
        name: "Ada".into(),
        created_at: Some(fixed()),
        updated_at: Some(fixed()),
    };
    let (sql, bindings) = build_update(&data, "postgres").unwrap();

    assert!(sql.contains("updated_at = $"), "got {sql}");
    assert!(
        !sql.contains("created_at"),
        "created_at is reserved and never reassigned: {sql}"
    );
    assert!(
        matches!(bindings.last(), Some(Value::Timestamp(_))),
        "updated_at binds a fresh timestamp"
    );
}

/// Verifies the dialect governs the timestamp bind shape.
#[test]
fn timestamp_bind_shape_tracks_dialect() {
    assert!(matches!(timestamp_value(fixed(), "sqlite"), Value::Text(_)));
    assert!(matches!(
        timestamp_value(fixed(), "postgres"),
        Value::Timestamp(_)
    ));
    assert!(matches!(
        timestamp_value(fixed(), "mysql"),
        Value::Timestamp(_)
    ));
}

/// Verifies caller-supplied timestamps parse from RFC3339 and Unix seconds.
#[test]
fn provided_timestamp_reads_supported_shapes() {
    let object = serde_json::json!({
        "created_at": "2020-01-02T03:04:05Z",
        "updated_at": 1_577_934_245,
        "deleted_at": null,
    });
    assert_eq!(provided_timestamp(&object, "created_at"), Some(fixed()));
    assert_eq!(
        provided_timestamp(&object, "updated_at"),
        Some(DateTime::from_timestamp(1_577_934_245, 0).unwrap())
    );
    assert_eq!(provided_timestamp(&object, "deleted_at"), None);
    assert_eq!(provided_timestamp(&object, "missing"), None);
}
