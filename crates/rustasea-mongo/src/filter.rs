//! Pragmatic filter builder producing [`bson::Document`] queries.
//!
//! [`Filter`] is a thin, non-ORM helper: it composes the comparison operators
//! most applications need (`eq`, `ne`, `gt`, `gte`, `lt`, `lte`, `in`,
//! `nin`, `regex`, `exists`) into the `{ field: { $op: value } }` shape the
//! driver expects. Filters chain with [`Filter::and`]/[`Filter::or`], and an
//! empty filter serializes to `{}` (match all).

use bson::raw::CString;
use bson::{Bson, Document, Regex};
use serde::Serialize;

use crate::error::{MongoError, Result};

/// A composable MongoDB query filter.
///
/// ```
/// use rustasea_mongo::Filter;
/// use bson::doc;
///
/// let filter = Filter::new()
///     .eq("status", "active")
///     .gte("age", 18);
/// assert_eq!(filter.into_document(), doc! { "status": "active", "age": { "$gte": 18 } });
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    /// The accumulated query document.
    inner: Document,
}

impl Filter {
    /// Start an empty filter (matches every document).
    pub fn new() -> Self {
        Self {
            inner: Document::new(),
        }
    }

    /// Field equality (`{ field: value }`).
    pub fn eq(mut self, field: impl Into<String>, value: impl Serialize) -> Self {
        if let Ok(bson) = to_bson(&value) {
            self.inner.insert(field.into(), bson);
        }
        self
    }

    /// Field inequality (`{ field: { $ne: value } }`).
    pub fn ne(self, field: impl Into<String>, value: impl Serialize) -> Self {
        self.operator(field, "$ne", value)
    }

    /// Greater-than (`{ field: { $gt: value } }`).
    pub fn gt(self, field: impl Into<String>, value: impl Serialize) -> Self {
        self.operator(field, "$gt", value)
    }

    /// Greater-than-or-equal (`{ field: { $gte: value } }`).
    pub fn gte(self, field: impl Into<String>, value: impl Serialize) -> Self {
        self.operator(field, "$gte", value)
    }

    /// Less-than (`{ field: { $lt: value } }`).
    pub fn lt(self, field: impl Into<String>, value: impl Serialize) -> Self {
        self.operator(field, "$lt", value)
    }

    /// Less-than-or-equal (`{ field: { $lte: value } }`).
    pub fn lte(self, field: impl Into<String>, value: impl Serialize) -> Self {
        self.operator(field, "$lte", value)
    }

    /// Membership (`{ field: { $in: [values...] } }`).
    pub fn is_in<V: Serialize>(self, field: impl Into<String>, values: Vec<V>) -> Self {
        let array: Vec<Bson> = values.iter().filter_map(|v| to_bson(v).ok()).collect();
        self.operator(field, "$in", array)
    }

    /// Non-membership (`{ field: { $nin: [values...] } }`).
    pub fn not_in<V: Serialize>(self, field: impl Into<String>, values: Vec<V>) -> Self {
        let array: Vec<Bson> = values.iter().filter_map(|v| to_bson(v).ok()).collect();
        self.operator(field, "$nin", array)
    }

    /// Field presence (`{ field: { $exists: true|false } }`).
    pub fn exists(mut self, field: impl Into<String>, present: bool) -> Self {
        self.inner
            .insert(field.into(), Bson::Document(doc_op("$exists", present)));
        self
    }

    /// Regular-expression match (`{ field: { $regex: /pattern/ } }`).
    ///
    /// `options` is a BSON regex option string (e.g. `"i"` for case-insensitive).
    /// Fails with [`MongoError::Configuration`] when `pattern` or `options`
    /// contains an interior NUL byte.
    pub fn regex(mut self, field: impl Into<String>, pattern: &str, options: &str) -> Result<Self> {
        let regex = Regex {
            pattern: CString::try_from(pattern).map_err(|_| {
                MongoError::Configuration("regex pattern contains a NUL byte".into())
            })?,
            options: CString::try_from(options).map_err(|_| {
                MongoError::Configuration("regex options contain a NUL byte".into())
            })?,
        };
        self.inner
            .insert(field.into(), Bson::RegularExpression(regex));
        Ok(self)
    }

    /// Combine two filters with `$and`.
    pub fn and(mut self, other: Filter) -> Self {
        self.merge("$and", other.inner);
        self
    }

    /// Combine two filters with `$or`.
    pub fn or(mut self, other: Filter) -> Self {
        self.merge("$or", other.inner);
        self
    }

    /// True when no conditions have been added (matches everything).
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Consume the builder and return the query document.
    pub fn into_document(self) -> Document {
        self.inner
    }

    /// Borrow the query document without consuming the builder.
    pub fn as_document(&self) -> &Document {
        &self.inner
    }

    /// Insert `{ field: { $op: value } }`, merging into an existing field clause.
    fn operator(mut self, field: impl Into<String>, op: &str, value: impl Serialize) -> Self {
        if let Ok(bson) = to_bson(&value) {
            let field = field.into();
            match self.inner.get_mut(&field) {
                Some(Bson::Document(existing)) => {
                    existing.insert(op.to_string(), bson);
                }
                _ => {
                    self.inner.insert(field, Bson::Document(doc_op(op, bson)));
                }
            }
        }
        self
    }

    /// Merge `other` under `op`, folding into an existing clause when present.
    fn merge(&mut self, op: &str, other: Document) {
        match self.inner.get_mut(op) {
            Some(Bson::Array(array)) => array.push(Bson::Document(other)),
            _ => {
                self.inner
                    .insert(op.to_string(), Bson::Array(vec![Bson::Document(other)]));
            }
        }
    }
}

impl From<Filter> for Document {
    /// Convert the filter into its query document.
    fn from(filter: Filter) -> Self {
        filter.inner
    }
}

/// Serialize a value into a [`Bson`], mapping failures to a typed error.
fn to_bson<T: Serialize>(value: &T) -> Result<Bson> {
    bson::serialize_to_bson(value).map_err(|e| MongoError::Serialization(e.to_string()))
}

/// Build a single-key `{ $op: value }` document.
fn doc_op(op: &str, value: impl Into<Bson>) -> Document {
    let mut doc = Document::new();
    doc.insert(op.to_string(), value.into());
    doc
}
