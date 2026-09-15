//! Cell coercion helpers — CSV text ↔ JSON values, and JSON → export cells.
//!
//! CSV has no types: every field arrives as text. [`coerce_csv_cell`] applies a
//! conservative, round-trip-safe inference so `"007"` stays a string while
//! `"42"` becomes an integer and `"3.5"` a float. XLSX cells are already typed
//! and are mapped directly by the XLSX reader. Exporting reverses the process
//! through [`ExportCell`], which preserves integer/float/bool/text distinctions
//! for XLSX while rendering every value to text for CSV.

use serde_json::{Number, Value};

/// Infer a JSON value from a raw CSV cell.
///
/// Rules (checked in order):
/// * empty string → [`Value::Null`];
/// * `"true"`/`"false"` (exact, lowercase) → boolean;
/// * an integer whose `to_string()` reproduces the cell exactly → number
///   (so `"007"` and `"+1"` stay strings);
/// * a finite float containing `.`, `e`, or `E` → number (so `"1.0"`, `"2.00"`,
///   `"1e3"`, and `"-0.5"` become numbers even though `f64::to_string` would
///   drop the decimal marker);
/// * otherwise the original string.
pub fn coerce_csv_cell(cell: &str) -> Value {
    if cell.is_empty() {
        return Value::Null;
    }
    match cell {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        _ => {}
    }
    if let Ok(int) = cell.parse::<i64>() {
        // Round-trip guard keeps leading-zero / signed-format strings intact.
        if int.to_string() == cell {
            return Value::Number(Number::from(int));
        }
    }
    // The presence of `.`/`e`/`E` distinguishes a float literal from an
    // integer-shaped string like `"007"`, which must stay text.
    if cell.contains(['.', 'e', 'E']) {
        if let Ok(float) = cell.parse::<f64>() {
            if let Some(num) = Number::from_f64(float) {
                return Value::Number(num);
            }
        }
    }
    Value::String(cell.to_string())
}

/// A typed export cell, preserving the value's kind for XLSX writers.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportCell {
    /// Text (also used for stringified arrays/objects).
    Text(String),
    /// A 64-bit integer cell.
    Integer(i64),
    /// A floating-point cell.
    Float(f64),
    /// A boolean cell.
    Bool(bool),
    /// An empty cell.
    Blank,
}

impl ExportCell {
    /// Render the cell to the text a CSV field expects (`None` → blank).
    pub fn to_csv_field(&self) -> Option<String> {
        match self {
            ExportCell::Text(s) => Some(s.clone()),
            ExportCell::Integer(i) => Some(i.to_string()),
            ExportCell::Float(f) => Some(f.to_string()),
            ExportCell::Bool(b) => Some(b.to_string()),
            ExportCell::Blank => None,
        }
    }
}

/// Map a JSON value onto an [`ExportCell`].
///
/// Strings stay text, integers/floats stay numeric, booleans stay boolean, null
/// becomes a blank cell, and arrays/objects are serialized to a JSON string so
/// no structured data is silently dropped.
pub fn value_to_export_cell(value: &Value) -> ExportCell {
    match value {
        Value::Null => ExportCell::Blank,
        Value::Bool(b) => ExportCell::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                ExportCell::Integer(i)
            } else if let Some(f) = n.as_f64() {
                ExportCell::Float(f)
            } else {
                ExportCell::Text(n.to_string())
            }
        }
        Value::String(s) => ExportCell::Text(s.clone()),
        other => ExportCell::Text(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Integer inference round-trips, preserving leading-zero strings.
    #[test]
    fn csv_integer_inference_is_round_trip_safe() {
        assert_eq!(coerce_csv_cell("42"), json!(42));
        assert_eq!(coerce_csv_cell("-7"), json!(-7));
        assert_eq!(coerce_csv_cell("007"), json!("007"));
        assert_eq!(coerce_csv_cell("+1"), json!("+1"));
    }

    /// Float inference keys off a decimal/exponent marker, not a `to_string`
    /// round-trip, so integral-float literals still coerce to numbers.
    #[test]
    fn csv_float_inference() {
        assert_eq!(coerce_csv_cell("3.5"), json!(3.5));
        assert_eq!(coerce_csv_cell("1.0"), json!(1.0));
        assert_eq!(coerce_csv_cell("2.00"), json!(2.0));
        assert_eq!(coerce_csv_cell("1e3"), json!(1000.0));
        assert_eq!(coerce_csv_cell("-0.5"), json!(-0.5));
    }

    /// Typed `f64` deserialization succeeds for integral-float CSV cells.
    #[test]
    fn integral_float_deserializes_into_f64() {
        #[derive(serde::Deserialize, Debug, PartialEq)]
        struct Score {
            /// The numeric column.
            score: f64,
        }
        let value = coerce_csv_cell("1.0");
        let parsed: Score = serde_json::from_value(json!({"score": value})).unwrap();
        assert_eq!(parsed.score, 1.0);
        let parsed: Score =
            serde_json::from_value(json!({"score": coerce_csv_cell("2.00")})).unwrap();
        assert_eq!(parsed.score, 2.0);
        let parsed: Score =
            serde_json::from_value(json!({"score": coerce_csv_cell("1e3")})).unwrap();
        assert_eq!(parsed.score, 1000.0);
    }

    /// Integer-shaped cells stay integers; leading-zero strings stay text.
    #[test]
    fn integer_cells_stay_integral() {
        assert_eq!(coerce_csv_cell("42"), json!(42));
        assert!(coerce_csv_cell("42").is_i64());
        assert_eq!(coerce_csv_cell("007"), json!("007"));
    }

    /// Booleans, empty cells, and plain strings map as documented.
    #[test]
    fn csv_bool_null_string() {
        assert_eq!(coerce_csv_cell("true"), json!(true));
        assert_eq!(coerce_csv_cell("false"), json!(false));
        assert_eq!(coerce_csv_cell(""), Value::Null);
        assert_eq!(coerce_csv_cell("hello"), json!("hello"));
    }

    /// Export cells preserve kind; structured values become JSON text.
    #[test]
    fn export_cell_mapping() {
        assert_eq!(value_to_export_cell(&json!(5)), ExportCell::Integer(5));
        assert_eq!(value_to_export_cell(&json!(5.5)), ExportCell::Float(5.5));
        assert_eq!(value_to_export_cell(&json!(true)), ExportCell::Bool(true));
        assert_eq!(value_to_export_cell(&json!(null)), ExportCell::Blank);
        assert_eq!(
            value_to_export_cell(&json!({"a": 1})),
            ExportCell::Text("{\"a\":1}".to_string())
        );
    }

    /// `to_csv_field` renders each variant, blank for null.
    #[test]
    fn export_cell_csv_rendering() {
        assert_eq!(ExportCell::Integer(3).to_csv_field(), Some("3".into()));
        assert_eq!(ExportCell::Bool(false).to_csv_field(), Some("false".into()));
        assert_eq!(ExportCell::Blank.to_csv_field(), None);
    }
}
