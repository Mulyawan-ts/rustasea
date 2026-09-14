//! Attribute casting — Eloquent-style `CastsAttributes` conversion of columns.
//!
//! A cast sits on the boundary between a column's database representation and
//! the Rust representation held by a model field. The derive macro records the
//! declared casts through [`Model::casts`](crate::model::Model::casts); the ORM
//! then applies them on the two crossing points:
//!
//! - **Hydration** ([`hydrate`]): a raw row is decoded, every cast column is
//!   converted to its Rust-facing JSON shape, then the row deserializes into
//!   the model.
//! - **Persistence** (`ModelOps`): the serialized model is converted back to
//!   bind values before the INSERT/UPDATE is built.
//!
//! Built-in casts live in [`builtin`] (`boolean`, `integer`, `float`, `string`,
//! `datetime`, `json`) and [`encrypted`] (`encrypted`). Custom casts implement
//! [`CastsAttributes`] for their target type and are wired with
//! `#[model(cast_with = "MyCast")]`.

pub mod builtin;
pub mod encrypted;

pub use builtin::{BooleanCast, DateTimeCast, FloatCast, IntegerCast, JsonCast, StringCast};
pub use encrypted::EncryptedCast;

use crate::error::{OrmError, Result};
use crate::model::Model;
use crate::types::Value;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Convert a single attribute between its database and Rust representations.
///
/// `T` is the Rust type held by the model field. [`get`](CastsAttributes::get)
/// receives the raw column value and returns `T`; [`set`](CastsAttributes::set)
/// receives a `T` and returns the bindable column value. A malformed or
/// out-of-range input must return [`OrmError::CastError`] rather than a silent
/// default.
pub trait CastsAttributes<T> {
    /// Convert a raw database value into the Rust type.
    fn get(&self, key: &str, value: &Value) -> Result<T>;

    /// Convert a Rust value into the bindable database representation.
    fn set(&self, key: &str, value: &T) -> Result<Value>;
}

/// Adapts a non-nullable cast to a nullable (`Option<T>`) column.
///
/// `NULL` hydrates to `None` and `None` persists as `NULL`; any non-null value
/// delegates to the wrapped cast. The derive macro wraps every cast declared on
/// an `Option<...>` field with this adapter.
pub struct NullableCast<C>(pub C);

impl<C, T> CastsAttributes<Option<T>> for NullableCast<C>
where
    C: CastsAttributes<T>,
{
    /// Hydrate `NULL` to `None`, delegating otherwise.
    fn get(&self, key: &str, value: &Value) -> Result<Option<T>> {
        if matches!(value, Value::Null) {
            return Ok(None);
        }
        self.0.get(key, value).map(Some)
    }

    /// Persist `None` as `NULL`, delegating otherwise.
    fn set(&self, key: &str, value: &Option<T>) -> Result<Value> {
        match value {
            Some(inner) => self.0.set(key, inner),
            None => Ok(Value::Null),
        }
    }
}

/// A resolved per-column cast binding emitted by `#[derive(Model)]`.
///
/// The two function pointers bridge the cast's typed [`CastsAttributes`] impl
/// and the ORM's JSON row pipeline without exposing `serde_json` to model
/// crates: `get` turns a raw bind value into the field's serde JSON shape and
/// `set` turns the serialized field back into a bind value.
pub struct CastBinding {
    /// Column the cast applies to.
    pub column: &'static str,
    /// Hydration direction: raw bind value → field's serde JSON representation.
    pub get: fn(&str, Value) -> Result<serde_json::Value>,
    /// Persistence direction: field's serde JSON representation → bind value.
    pub set: fn(&str, &serde_json::Value) -> Result<Value>,
}

/// Build a typed [`OrmError::CastError`] for a failed cast.
pub fn cast_error(column: &str, message: impl Into<String>) -> OrmError {
    OrmError::CastError {
        column: column.to_string(),
        message: message.into(),
    }
}

/// Serialize a cast value into the field's serde JSON representation.
///
/// Used by the derive-generated hydration closure so model crates never depend
/// on `serde_json` directly.
pub fn serialize_to_json<T: Serialize>(key: &str, value: &T) -> Result<serde_json::Value> {
    serde_json::to_value(value).map_err(|error| cast_error(key, error.to_string()))
}

/// Deserialize a field's serde JSON representation into the cast target type.
///
/// Used by the derive-generated persistence closure so model crates never
/// depend on `serde_json` directly.
pub fn deserialize_from_json<T: DeserializeOwned>(
    key: &str,
    value: &serde_json::Value,
) -> Result<T> {
    serde_json::from_value(value.clone()).map_err(|error| cast_error(key, error.to_string()))
}

/// Hydrate a raw database row into a model, applying every declared cast.
///
/// Each cast column present in the row is converted to its Rust-facing JSON
/// shape before the row deserializes, so a `#[model(cast = "json")]` field
/// receives its parsed value and corrupt input surfaces as
/// [`OrmError::CastError`].
pub fn hydrate<M>(row: serde_json::Value) -> Result<M>
where
    M: Model + DeserializeOwned,
{
    let row = apply_get_casts::<M>(row)?;
    crate::builder::json_to_model(row)
}

/// Apply the hydration direction of every cast declared on `M` to `row`.
fn apply_get_casts<M>(mut row: serde_json::Value) -> Result<serde_json::Value>
where
    M: Model,
{
    let bindings = M::casts();
    if bindings.is_empty() {
        return Ok(row);
    }
    let object = row
        .as_object_mut()
        .ok_or_else(|| OrmError::InvalidValue("model row must be a JSON object".into()))?;
    for binding in &bindings {
        if let Some(raw) = object.get(binding.column).cloned() {
            let value = Value::from_json(&raw);
            let casted = (binding.get)(binding.column, value)?;
            object.insert(binding.column.to_string(), casted);
        }
    }
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    /// A JSON-backed target proving the binding round-trips through serde.
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Settings {
        theme: String,
    }

    /// Verifies the JSON cast binding hydrates a text column into a struct.
    #[test]
    fn binding_hydrates_json_text() {
        let binding = CastBinding {
            column: "settings",
            get: |key, value| {
                let cast = JsonCast;
                let typed: Settings = cast.get(key, &value)?;
                serialize_to_json(key, &typed)
            },
            set: |key, value| {
                let cast = JsonCast;
                let typed: Settings = deserialize_from_json(key, value)?;
                cast.set(key, &typed)
            },
        };

        let raw = Value::Text(r#"{"theme":"dark"}"#.to_string());
        let json = (binding.get)(binding.column, raw).unwrap();
        let parsed: Settings = deserialize_from_json(binding.column, &json).unwrap();
        assert_eq!(
            parsed,
            Settings {
                theme: "dark".into()
            }
        );

        let bound = (binding.set)(binding.column, &json).unwrap();
        assert!(matches!(bound, Value::Json(_)));
    }

    /// Verifies a corrupt JSON payload surfaces as a typed cast error.
    #[test]
    fn corrupt_json_is_cast_error() {
        let error = <JsonCast as CastsAttributes<Settings>>::get(
            &JsonCast,
            "settings",
            &Value::Text("not-json".into()),
        )
        .expect_err("corrupt JSON must fail");
        assert!(matches!(error, OrmError::CastError { .. }), "got {error:?}");
    }

    /// Verifies `Value` JSON round-trips through `from_json`/`to_json`.
    #[test]
    fn value_json_round_trip() {
        let raw = serde_json::json!({"a": 1, "b": [true, null]});
        assert_eq!(Value::from_json(&raw).to_json(), raw);
    }
}
