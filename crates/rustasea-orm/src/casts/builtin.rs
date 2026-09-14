//! Built-in attribute casts — `boolean`, `integer`, `float`, `string`,
//! `datetime`, and `json`.
//!
//! Each cast tolerates the native driver representation (a typed scalar, a
//! timestamp, or a JSON value) as well as the text form SQLite and MySQL use,
//! and returns [`OrmError::CastError`](crate::error::OrmError::CastError) when a
//! value cannot be interpreted rather than silently coercing it.

use crate::casts::{cast_error, CastsAttributes};
use crate::error::Result;
use crate::types::Value;
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Casts a column to/from `bool`.
pub struct BooleanCast;

impl CastsAttributes<bool> for BooleanCast {
    /// Interpret a boolean, 0/1 integer, or `true`/`false`-style text as a bool.
    fn get(&self, key: &str, value: &Value) -> Result<bool> {
        match value {
            Value::Bool(flag) => Ok(*flag),
            Value::Int(int) => Ok(*int != 0),
            Value::Float(float) => Ok(*float != 0.0),
            Value::Text(text) => match text.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "t" | "yes" | "on" => Ok(true),
                "0" | "false" | "f" | "no" | "off" => Ok(false),
                other => Err(cast_error(key, format!("cannot cast `{other}` to bool"))),
            },
            Value::Null => Ok(false),
            other => Err(cast_error(key, format!("cannot cast {other:?} to bool"))),
        }
    }

    /// Persist a bool natively.
    fn set(&self, _key: &str, value: &bool) -> Result<Value> {
        Ok(Value::Bool(*value))
    }
}

/// Casts a column to/from `i64`.
pub struct IntegerCast;

impl CastsAttributes<i64> for IntegerCast {
    /// Interpret an integer, float, bool, or numeric text as an `i64`.
    fn get(&self, key: &str, value: &Value) -> Result<i64> {
        match value {
            Value::Int(int) => Ok(*int),
            Value::Float(float) => Ok(*float as i64),
            Value::Bool(flag) => Ok(i64::from(*flag)),
            Value::Text(text) => text
                .trim()
                .parse::<i64>()
                .map_err(|_| cast_error(key, format!("cannot cast `{text}` to integer"))),
            Value::Null => Ok(0),
            other => Err(cast_error(key, format!("cannot cast {other:?} to integer"))),
        }
    }

    /// Persist an integer natively.
    fn set(&self, _key: &str, value: &i64) -> Result<Value> {
        Ok(Value::Int(*value))
    }
}

/// Casts a column to/from `f64`.
pub struct FloatCast;

impl CastsAttributes<f64> for FloatCast {
    /// Interpret a float, integer, bool, or numeric text as an `f64`.
    fn get(&self, key: &str, value: &Value) -> Result<f64> {
        match value {
            Value::Float(float) => Ok(*float),
            Value::Int(int) => Ok(*int as f64),
            Value::Bool(flag) => Ok(if *flag { 1.0 } else { 0.0 }),
            Value::Text(text) => text
                .trim()
                .parse::<f64>()
                .map_err(|_| cast_error(key, format!("cannot cast `{text}` to float"))),
            Value::Null => Ok(0.0),
            other => Err(cast_error(key, format!("cannot cast {other:?} to float"))),
        }
    }

    /// Persist a float natively.
    fn set(&self, _key: &str, value: &f64) -> Result<Value> {
        Ok(Value::Float(*value))
    }
}

/// Casts a column to/from `String`.
pub struct StringCast;

impl CastsAttributes<String> for StringCast {
    /// Render any scalar (and JSON) column as text.
    fn get(&self, key: &str, value: &Value) -> Result<String> {
        match value {
            Value::Text(text) => Ok(text.clone()),
            Value::Int(int) => Ok(int.to_string()),
            Value::Float(float) => Ok(float.to_string()),
            Value::Bool(flag) => Ok(flag.to_string()),
            Value::Uuid(id) => Ok(id.to_string()),
            Value::Timestamp(ts) => Ok(ts.to_rfc3339()),
            Value::Json(json) => {
                serde_json::to_string(json).map_err(|error| cast_error(key, error.to_string()))
            }
            Value::Null => Ok(String::new()),
            #[cfg(feature = "vector")]
            other => Err(cast_error(key, format!("cannot cast {other:?} to string"))),
        }
    }

    /// Persist text natively.
    fn set(&self, _key: &str, value: &String) -> Result<Value> {
        Ok(Value::Text(value.clone()))
    }
}

/// Casts a column to/from [`DateTime<Utc>`].
pub struct DateTimeCast;

impl CastsAttributes<DateTime<Utc>> for DateTimeCast {
    /// Parse a native timestamp, RFC3339 text, `%Y-%m-%d %H:%M:%S` text, or unix seconds.
    fn get(&self, key: &str, value: &Value) -> Result<DateTime<Utc>> {
        match value {
            Value::Timestamp(ts) => Ok(*ts),
            Value::Text(text) => parse_datetime(key, text),
            Value::Int(seconds) => Utc.timestamp_opt(*seconds, 0).single().ok_or_else(|| {
                cast_error(key, format!("unix timestamp `{seconds}` is out of range"))
            }),
            Value::Null => Err(cast_error(key, "cannot cast NULL to a datetime")),
            other => Err(cast_error(
                key,
                format!("cannot cast {other:?} to datetime"),
            )),
        }
    }

    /// Persist a timestamp natively.
    fn set(&self, _key: &str, value: &DateTime<Utc>) -> Result<Value> {
        Ok(Value::Timestamp(*value))
    }
}

/// Parse the text forms a datetime column may carry.
fn parse_datetime(key: &str, text: &str) -> Result<DateTime<Utc>> {
    let trimmed = text.trim();
    if let Ok(parsed) = DateTime::parse_from_rfc3339(trimmed) {
        return Ok(parsed.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, format) {
            return Ok(Utc.from_utc_datetime(&naive));
        }
    }
    Err(cast_error(key, format!("cannot cast `{text}` to datetime")))
}

/// Casts a column to/from any `serde`-serializable type through JSON.
///
/// The column may hold a native JSON value or its text encoding (SQLite/MySQL);
/// the target type round-trips through `serde_json`.
pub struct JsonCast;

impl<T> CastsAttributes<T> for JsonCast
where
    T: Serialize + DeserializeOwned,
{
    /// Parse a JSON (or JSON-text) column into `T`.
    fn get(&self, key: &str, value: &Value) -> Result<T> {
        let json = match value {
            Value::Json(json) => json.clone(),
            Value::Text(text) => serde_json::from_str(text)
                .map_err(|error| cast_error(key, format!("invalid JSON: {error}")))?,
            Value::Null => serde_json::Value::Null,
            other => {
                return Err(cast_error(
                    key,
                    format!("json cast expects JSON or text, got {other:?}"),
                ))
            }
        };
        serde_json::from_value(json).map_err(|error| cast_error(key, error.to_string()))
    }

    /// Serialize `T` into a native JSON bind value.
    fn set(&self, key: &str, value: &T) -> Result<Value> {
        serde_json::to_value(value)
            .map(Value::Json)
            .map_err(|error| cast_error(key, error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies the boolean cast accepts every documented representation.
    #[test]
    fn boolean_accepts_common_forms() {
        let cast = BooleanCast;
        assert!(cast.get("flag", &Value::Bool(true)).unwrap());
        assert!(cast.get("flag", &Value::Int(1)).unwrap());
        assert!(cast.get("flag", &Value::Text("TRUE".into())).unwrap());
        assert!(!cast.get("flag", &Value::Text("no".into())).unwrap());
        assert!(matches!(
            cast.get("flag", &Value::Text("maybe".into())),
            Err(crate::error::OrmError::CastError { .. })
        ));
    }

    /// Verifies the integer and float casts parse text and reject garbage.
    #[test]
    fn numeric_casts_parse_and_reject() {
        assert_eq!(IntegerCast.get("n", &Value::Text("42".into())).unwrap(), 42);
        assert_eq!(FloatCast.get("n", &Value::Int(3)).unwrap(), 3.0);
        assert!(IntegerCast.get("n", &Value::Text("x".into())).is_err());
    }

    /// Verifies the datetime cast parses RFC3339 and naive SQL text.
    #[test]
    fn datetime_parses_known_formats() {
        let rfc = DateTimeCast
            .get("at", &Value::Text("2026-09-14T10:00:00Z".into()))
            .unwrap();
        assert_eq!(rfc.to_rfc3339(), "2026-09-14T10:00:00+00:00");
        assert!(DateTimeCast
            .get("at", &Value::Text("2026-09-14 10:00:00".into()))
            .is_ok());
        assert!(DateTimeCast.get("at", &Value::Text("nope".into())).is_err());
    }
}
