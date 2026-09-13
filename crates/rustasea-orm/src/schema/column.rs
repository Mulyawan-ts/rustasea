//! Column model for the schema builder.
//!
//! A [`Column`] pairs a logical [`ColumnKind`] with the fluent modifiers
//! (`nullable`, `unique`, `index`, `default`) a migration author chains onto it.
//! The dialect-specific SQL type is *not* stored here — it is resolved at
//! emission time by [`super::emit`] from the runtime dialect string, so one
//! blueprint renders valid DDL for SQLite, Postgres, and MySQL alike.

/// The logical type of a [`Column`], independent of any database dialect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnKind {
    /// Auto-incrementing 64-bit primary key (`id`, `big_increments`).
    Increments,
    /// Variable-length string with an explicit maximum length.
    String(u32),
    /// Unbounded text.
    Text,
    /// 32-bit integer.
    Integer,
    /// 64-bit integer.
    BigInteger,
    /// Boolean flag.
    Boolean,
    /// Timestamp with time zone.
    Timestamp,
    /// Fixed-precision decimal (`NUMERIC`/`DECIMAL`).
    Decimal {
        /// Total number of significant digits.
        precision: u32,
        /// Digits to the right of the decimal point.
        scale: u32,
    },
    /// JSON document.
    Json,
    /// UUID value.
    Uuid,
    /// Unsigned 64-bit foreign key (no auto-increment).
    ForeignId,
}

/// A literal `DEFAULT` value attached to a column.
///
/// Values are rendered per dialect (e.g. booleans become `1`/`0` on SQLite and
/// `TRUE`/`FALSE` on Postgres). Use [`DefaultValue::Raw`] for a driver
/// expression such as `CURRENT_TIMESTAMP`.
#[derive(Debug, Clone, PartialEq)]
pub enum DefaultValue {
    /// Explicit `DEFAULT NULL`.
    Null,
    /// Boolean literal.
    Bool(bool),
    /// Integer literal.
    Int(i64),
    /// Floating-point literal.
    Float(f64),
    /// Quoted string literal (single quotes are escaped).
    Text(String),
    /// Raw SQL expression emitted verbatim (never interpolate user input).
    Raw(String),
}

impl From<bool> for DefaultValue {
    /// Wrap a boolean literal.
    fn from(value: bool) -> Self {
        DefaultValue::Bool(value)
    }
}

impl From<i32> for DefaultValue {
    /// Wrap a 32-bit integer literal.
    fn from(value: i32) -> Self {
        DefaultValue::Int(value as i64)
    }
}

impl From<i64> for DefaultValue {
    /// Wrap a 64-bit integer literal.
    fn from(value: i64) -> Self {
        DefaultValue::Int(value)
    }
}

impl From<u32> for DefaultValue {
    /// Wrap an unsigned 32-bit integer literal.
    fn from(value: u32) -> Self {
        DefaultValue::Int(i64::from(value))
    }
}

impl From<f64> for DefaultValue {
    /// Wrap a floating-point literal.
    fn from(value: f64) -> Self {
        DefaultValue::Float(value)
    }
}

impl From<&str> for DefaultValue {
    /// Wrap a borrowed string literal.
    fn from(value: &str) -> Self {
        DefaultValue::Text(value.to_string())
    }
}

impl From<String> for DefaultValue {
    /// Wrap an owned string literal.
    fn from(value: String) -> Self {
        DefaultValue::Text(value)
    }
}

/// A single column definition inside a [`super::Blueprint`].
///
/// Build one through the blueprint's typed helpers (`id`, `string`, `text`, …)
/// and refine it with the fluent modifiers ([`Column::nullable`],
/// [`Column::unique`], [`Column::index`], [`Column::default`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    /// Column name, emitted unquoted (must be a valid identifier).
    pub(crate) name: String,
    /// Logical column type.
    pub(crate) kind: ColumnKind,
    /// Whether the column accepts `NULL` (default: not nullable).
    pub(crate) nullable: bool,
    /// Optional `DEFAULT` literal.
    pub(crate) default: Option<DefaultValue>,
    /// Whether the column is (part of) the primary key.
    pub(crate) primary: bool,
    /// Whether a unique index should be emitted for this column.
    pub(crate) unique: bool,
    /// Whether a plain index should be emitted for this column.
    pub(crate) index: bool,
    /// Whether the column auto-increments (set only by [`ColumnKind::Increments`]).
    pub(crate) auto_increment: bool,
}

impl Column {
    /// Create a plain, non-nullable column of `kind`.
    pub(crate) fn new(name: &str, kind: ColumnKind) -> Self {
        Self {
            name: name.to_string(),
            kind,
            nullable: false,
            default: None,
            primary: false,
            unique: false,
            index: false,
            auto_increment: false,
        }
    }

    /// Create an auto-incrementing primary-key column (`id`/`big_increments`).
    pub(crate) fn increments(name: &str) -> Self {
        Self {
            name: name.to_string(),
            kind: ColumnKind::Increments,
            nullable: false,
            default: None,
            primary: true,
            unique: false,
            index: false,
            auto_increment: true,
        }
    }

    /// The column name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The logical column type.
    pub fn kind(&self) -> &ColumnKind {
        &self.kind
    }

    /// Whether the column accepts `NULL`.
    pub fn is_nullable(&self) -> bool {
        self.nullable
    }

    /// Whether a unique index is emitted for this column.
    pub fn is_unique(&self) -> bool {
        self.unique
    }

    /// Whether a plain index is emitted for this column.
    pub fn is_indexed(&self) -> bool {
        self.index
    }

    /// Whether the column auto-increments.
    pub fn is_auto_increment(&self) -> bool {
        self.auto_increment
    }

    /// Allow `NULL` values in this column.
    pub fn nullable(&mut self) -> &mut Self {
        self.nullable = true;
        self
    }

    /// Emit a unique index for this column.
    pub fn unique(&mut self) -> &mut Self {
        self.unique = true;
        self
    }

    /// Emit a plain (non-unique) index for this column.
    pub fn index(&mut self) -> &mut Self {
        self.index = true;
        self
    }

    /// Set the column's `DEFAULT` literal.
    pub fn default<V: Into<DefaultValue>>(&mut self, value: V) -> &mut Self {
        self.default = Some(value.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies `From` conversions land on the expected `DefaultValue` variants.
    #[test]
    fn default_value_conversions() {
        assert_eq!(DefaultValue::from(true), DefaultValue::Bool(true));
        assert_eq!(DefaultValue::from(7_i64), DefaultValue::Int(7));
        assert_eq!(DefaultValue::from(7_u32), DefaultValue::Int(7));
        assert_eq!(DefaultValue::from("x"), DefaultValue::Text("x".into()));
        assert_eq!(DefaultValue::from(1.5_f64), DefaultValue::Float(1.5));
    }

    /// Verifies modifiers set the flags the emitters read.
    #[test]
    fn modifiers_toggle_flags() {
        let mut column = Column::new("email", ColumnKind::String(255));
        column.nullable().unique().index().default("none");
        assert!(column.is_nullable());
        assert!(column.is_unique());
        assert!(column.is_indexed());
        assert_eq!(column.default, Some(DefaultValue::Text("none".into())));
    }

    /// Verifies the increments helper marks a column primary and auto-increment.
    #[test]
    fn increments_is_primary() {
        let column = Column::increments("id");
        assert!(column.is_auto_increment());
        assert!(column.primary);
        assert_eq!(column.kind(), &ColumnKind::Increments);
    }
}
