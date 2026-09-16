//! Bind values and JSON filter primitives shared by builder and model layers.

use crate::error::{OrmError, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

/// A single bind value for a prepared statement.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// NULL.
    Null,
    /// Boolean.
    Bool(bool),
    /// 64-bit integer.
    Int(i64),
    /// 64-bit float (never used for monetary values — `Decimal` in M4).
    Float(f64),
    /// UTF-8 text.
    Text(String),
    /// UUID.
    Uuid(uuid::Uuid),
    /// Timestamp with time zone, bound natively by the driver.
    Timestamp(DateTime<Utc>),
    /// JSON value.
    Json(serde_json::Value),
    /// Vector of floats (pgvector embedding).
    #[cfg(feature = "vector")]
    Vector(Vec<f32>),
}

impl Value {
    /// Build a bind value from a JSON scalar.
    ///
    /// Objects and arrays become [`Value::Json`]; integers prefer [`Value::Int`]
    /// and fall back to [`Value::Float`]. Used by the cast hydration boundary to
    /// reinterpret a decoded row cell as a bind value.
    pub fn from_json(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(flag) => Value::Bool(*flag),
            serde_json::Value::Number(number) => match number.as_i64() {
                Some(int) => Value::Int(int),
                None => Value::Float(number.as_f64().unwrap_or_default()),
            },
            serde_json::Value::String(text) => Value::Text(text.clone()),
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                Value::Json(value.clone())
            }
        }
    }

    /// Render this bind value back to JSON (inverse of [`Value::from_json`]).
    ///
    /// Timestamps render as RFC3339 strings and UUIDs as canonical text so the
    /// JSON representation is stable across drivers.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Value::Null => serde_json::Value::Null,
            Value::Bool(flag) => serde_json::Value::Bool(*flag),
            Value::Int(int) => serde_json::Value::from(*int),
            Value::Float(float) => serde_json::Value::from(*float),
            Value::Text(text) => serde_json::Value::String(text.clone()),
            Value::Uuid(id) => serde_json::Value::String(id.to_string()),
            Value::Timestamp(ts) => {
                serde_json::Value::String(ts.to_rfc3339_opts(SecondsFormat::Micros, true))
            }
            Value::Json(json) => json.clone(),
            #[cfg(feature = "vector")]
            Value::Vector(vector) => serde_json::Value::Array(
                vector
                    .iter()
                    .map(|float| serde_json::Value::from(*float))
                    .collect(),
            ),
        }
    }

    /// Render the value as a literal for `toRawSql` diagnostics.
    ///
    /// Always parameterized in real execution — raw SQL is display-only.
    pub fn to_literal(&self) -> String {
        match self {
            Value::Null => "NULL".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => format!("{f}"),
            Value::Text(s) => format!("'{}'", s.replace('\'', "''")),
            Value::Uuid(u) => format!("'{u}'"),
            Value::Timestamp(t) => {
                format!("'{}'", t.to_rfc3339_opts(SecondsFormat::Micros, true))
            }
            Value::Json(v) => {
                format!(
                    "'{}'",
                    serde_json::to_string(v)
                        .unwrap_or_default()
                        .replace('\'', "''")
                )
            }
            #[cfg(feature = "vector")]
            Value::Vector(v) => {
                let parts: Vec<String> = v.iter().map(|f| f.to_string()).collect();
                format!("[{}]", parts.join(","))
            }
        }
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Text(s.to_string())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Text(s)
    }
}

impl From<i64> for Value {
    fn from(i: i64) -> Self {
        Value::Int(i)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<uuid::Uuid> for Value {
    fn from(u: uuid::Uuid) -> Self {
        Value::Uuid(u)
    }
}

impl From<f64> for Value {
    fn from(f: f64) -> Self {
        Value::Float(f)
    }
}

/// Filter operator applied to a JSON path expression.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonFilter {
    /// `path @> $n` — JSON contains the given value.
    Contains(serde_json::Value),
    /// `path ->> key = $n` — key equality against text.
    PathEquals(String, String),
    /// `json_exists(path)` / `path ? key` — key exists.
    KeyExists(String),
}

impl JsonFilter {
    /// Bind value for this filter, if any.
    ///
    /// `Contains` binds the contained JSON; `PathEquals` binds the expected text
    /// at the path; `KeyExists` has no bind value.
    pub fn bind_value(&self) -> Option<Value> {
        match self {
            JsonFilter::Contains(value) => Some(Value::Json(value.clone())),
            JsonFilter::PathEquals(_, value) => Some(Value::Text(value.clone())),
            JsonFilter::KeyExists(_) => None,
        }
    }

    /// Compile the filter to a SQL fragment for the given driver dialect.
    pub fn to_sql(&self, column: &str, dialect: &str) -> Result<String> {
        match self {
            JsonFilter::Contains(_) => match dialect {
                "postgres" => Ok(format!("{column} @> {{}}")),
                "mysql" => Ok(format!("JSON_CONTAINS({column}, {{}})")),
                "sqlite" => Err(OrmError::UnsupportedDriver(
                    "sqlite has no native JSON contains operator".into(),
                )),
                other => Err(OrmError::UnsupportedDriver(other.to_string())),
            },
            JsonFilter::PathEquals(key, _) => match dialect {
                "postgres" => Ok(format!("{column} ->> '{{{key}}}' = {{}}")),
                "mysql" => Ok(format!(
                    "JSON_UNQUOTE(JSON_EXTRACT({column}, '$.{key}')) = {{}}"
                )),
                "sqlite" => Ok(format!("json_extract({column}, '$.{key}') = {{}}")),
                other => Err(OrmError::UnsupportedDriver(other.to_string())),
            },
            JsonFilter::KeyExists(key) => match dialect {
                "postgres" => Ok(format!("{column} ? '{key}'")),
                "mysql" => Ok(format!("JSON_CONTAINS_PATH({column}, 'one', '$.{key}')")),
                "sqlite" => Ok(format!("json_type({column}, '$.{key}') IS NOT NULL")),
                other => Err(OrmError::UnsupportedDriver(other.to_string())),
            },
        }
    }
}

/// A JSON-embedded key set used by the `*_json` relation predicates (ADOPT-020).
///
/// `path` is the dot-path into the JSON document and `values` are the candidate
/// keys to match. Two semantics are supported, matching the two eager loaders:
///
/// * [`JsonRelationFilter::to_sql`] — **scalar IN**: the value at `path` is a
///   single key compared against `values` (`BelongsToJson`).
/// * [`JsonRelationFilter::to_sql_array`] — **array overlap**: the array at
///   `path` contains any of `values` (`HasManyJson` / `BelongsToManyJson`).
///
/// Both emit `$n` placeholders (rewritten to `?` for MySQL by `db/adapt.rs`)
/// and return the SQL fragment plus the number of bind values it consumes.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonRelationFilter {
    /// Dot-path into the JSON document (e.g. `author.id`).
    pub path: String,
    /// Candidate keys matched against the embedded value(s).
    pub values: Vec<String>,
}

impl JsonRelationFilter {
    /// Compile the **scalar** form: match the value at `path` against `values`.
    ///
    /// Returns the SQL fragment and the number of bind values it consumes
    /// (`values.len()`). Each bind site is a `{}` token; the builder substitutes
    /// the ordered `$n` placeholders. An empty `values` list is the caller's
    /// concern (the builder degrades it to `1 = 0`); this method still emits
    /// valid SQL.
    pub fn to_sql(&self, column: &str, dialect: &str) -> Result<(String, usize)> {
        let path = escape_path(&self.path);
        let count = self.values.len();
        let sql = match dialect {
            "postgres" => format!("({column} ->> '{path}') IN {}", placeholders(count)),
            "mysql" => format!(
                "JSON_UNQUOTE(JSON_EXTRACT({column}, '$.{path}')) IN {}",
                placeholders(count)
            ),
            "sqlite" => format!(
                "json_valid({column}) AND json_extract({column}, '$.{path}') IN {}",
                placeholders(count)
            ),
            other => return Err(OrmError::UnsupportedDriver(other.to_string())),
        };
        Ok((sql, count))
    }

    /// Compile the **array overlap** form: the array at `path` contains any of
    /// `values`.
    ///
    /// Postgres and SQLite bind one placeholder per value. MySQL binds a single
    /// JSON-array literal (the values are joined in Rust and bound as
    /// `CAST({} AS JSON)`), so the returned bind count is `1` even when several
    /// values are supplied — the caller must bind the joined array accordingly.
    pub fn to_sql_array(&self, column: &str, dialect: &str) -> Result<(String, usize)> {
        let path = escape_path(&self.path);
        match dialect {
            "postgres" => Ok((
                format!(
                    "(({column})::jsonb -> '{path}') ?| {}",
                    array_literal(self.values.len())
                ),
                self.values.len(),
            )),
            "mysql" => Ok((
                format!("JSON_OVERLAPS(JSON_EXTRACT({column}, '$.{path}'), CAST({{}} AS JSON))"),
                1,
            )),
            "sqlite" => Ok((
                format!(
                    "json_valid({column}) AND EXISTS (SELECT 1 FROM json_each({column}, '$.{path}') \
                     WHERE json_each.value IN {})",
                    placeholders(self.values.len())
                ),
                self.values.len(),
            )),
            other => Err(OrmError::UnsupportedDriver(other.to_string())),
        }
    }
}

/// Render `({}, {}, …)` with `n` `{}` bind tokens.
fn placeholders(n: usize) -> String {
    let list: Vec<&str> = std::iter::repeat_n("{}", n).collect();
    format!("({})", list.join(", "))
}

/// Render a Postgres `ARRAY[{}, {}, …]` literal with `n` `{}` bind tokens.
fn array_literal(n: usize) -> String {
    let list: Vec<&str> = std::iter::repeat_n("{}", n).collect();
    format!("ARRAY[{}]", list.join(", "))
}

/// Escape single quotes in a JSON path so it cannot break out of the literal.
fn escape_path(path: &str) -> String {
    path.replace('\'', "''")
}

/// Schema column type used by Blueprint/migration helpers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnType {
    /// UUID primary key.
    Uuid,
    /// Variable-length text.
    Text,
    /// 64-bit integer.
    BigInt,
    /// `NUMERIC(15,2)` — mandated for money/quantities.
    Decimal,
    /// Boolean flag.
    Boolean,
    /// `JSONB` (Postgres) / `JSON` (MySQL) / `TEXT` (SQLite).
    Json,
    /// Timestamp with time zone.
    TimestampTz,
    /// pgvector embedding column of fixed dimension.
    #[cfg(feature = "vector")]
    Vector(u32),
}

impl ColumnType {
    /// Emit the dialect-specific SQL type for this column.
    pub fn to_sql(&self, dialect: &str) -> Result<String> {
        match self {
            ColumnType::Uuid => Ok(match dialect {
                "postgres" => "UUID".to_string(),
                "mysql" => "CHAR(36)".to_string(),
                "sqlite" => "TEXT".to_string(),
                other => return Err(OrmError::UnsupportedDriver(other.to_string())),
            }),
            ColumnType::Text => Ok(match dialect {
                "postgres" => "TEXT".to_string(),
                "mysql" => "VARCHAR(255)".to_string(),
                "sqlite" => "TEXT".to_string(),
                other => return Err(OrmError::UnsupportedDriver(other.to_string())),
            }),
            ColumnType::BigInt => Ok(match dialect {
                "postgres" => "BIGINT".to_string(),
                "mysql" => "BIGINT".to_string(),
                "sqlite" => "INTEGER".to_string(),
                other => return Err(OrmError::UnsupportedDriver(other.to_string())),
            }),
            ColumnType::Decimal => Ok(match dialect {
                "postgres" => "NUMERIC(15,2)".to_string(),
                "mysql" => "DECIMAL(15,2)".to_string(),
                "sqlite" => "NUMERIC(15,2)".to_string(),
                other => return Err(OrmError::UnsupportedDriver(other.to_string())),
            }),
            ColumnType::Boolean => Ok(match dialect {
                "postgres" => "BOOLEAN".to_string(),
                "mysql" => "TINYINT(1)".to_string(),
                "sqlite" => "INTEGER".to_string(),
                other => return Err(OrmError::UnsupportedDriver(other.to_string())),
            }),
            ColumnType::Json => Ok(match dialect {
                "postgres" => "JSONB".to_string(),
                "mysql" => "JSON".to_string(),
                "sqlite" => "TEXT".to_string(),
                other => return Err(OrmError::UnsupportedDriver(other.to_string())),
            }),
            ColumnType::TimestampTz => Ok(match dialect {
                "postgres" => "TIMESTAMPTZ".to_string(),
                "mysql" => "DATETIME".to_string(),
                "sqlite" => "TEXT".to_string(),
                other => return Err(OrmError::UnsupportedDriver(other.to_string())),
            }),
            #[cfg(feature = "vector")]
            ColumnType::Vector(dim) => match dialect {
                "postgres" => Ok(format!("VECTOR({dim})")),
                "mysql" => Ok(format!("VECTOR({dim})")),
                other => Err(OrmError::UnsupportedDriver(format!(
                    "{other} does not support vector columns"
                ))),
            },
        }
    }
}
