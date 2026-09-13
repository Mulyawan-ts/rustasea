//! Pragmatic update builder producing [`bson::Document`] update documents.
//!
//! [`Update`] composes the common MongoDB update operators (`$set`, `$unset`,
//! `$inc`, `$push`, `$pull`) into the `{ $op: { field: value } }` shape the
//! driver expects. It is deliberately not an ORM: each call targets one field
//! path and no schema is tracked.

use bson::{Bson, Document};
use serde::Serialize;

use crate::error::{MongoError, Result};

/// A composable MongoDB update document.
///
/// ```
/// use rustasea_mongo::Update;
/// use bson::doc;
///
/// let update = Update::new().set("name", "Ada").inc("logins", 1);
/// assert_eq!(update.into_document(), doc! { "$set": { "name": "Ada" }, "$inc": { "logins": 1 } });
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Update {
    /// The accumulated update document.
    inner: Document,
}

impl Update {
    /// Start an empty update.
    pub fn new() -> Self {
        Self {
            inner: Document::new(),
        }
    }

    /// Set a field to a value (`$set`).
    pub fn set(self, field: impl Into<String>, value: impl Serialize) -> Self {
        self.stage("$set", field, value)
    }

    /// Remove a field (`$unset`).
    pub fn unset(self, field: impl Into<String>) -> Self {
        self.stage("$unset", field, "")
    }

    /// Increment a numeric field (`$inc`).
    pub fn inc(self, field: impl Into<String>, amount: impl Serialize) -> Self {
        self.stage("$inc", field, amount)
    }

    /// Append a value to an array field (`$push`).
    pub fn push(self, field: impl Into<String>, value: impl Serialize) -> Self {
        self.stage("$push", field, value)
    }

    /// Remove matching values from an array field (`$pull`).
    pub fn pull(self, field: impl Into<String>, value: impl Serialize) -> Self {
        self.stage("$pull", field, value)
    }

    /// True when no operators have been added.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Consume the builder and return the update document.
    pub fn into_document(self) -> Document {
        self.inner
    }

    /// Borrow the update document without consuming the builder.
    pub fn as_document(&self) -> &Document {
        &self.inner
    }

    /// Insert `{ field: value }` under the operator `op`, creating it if absent.
    fn stage(mut self, op: &str, field: impl Into<String>, value: impl Serialize) -> Self {
        if let Ok(bson) = to_bson(&value) {
            let entry = self
                .inner
                .entry(op.to_string())
                .or_insert_with(|| Bson::Document(Document::new()));
            if let Bson::Document(doc) = entry {
                doc.insert(field.into(), bson);
            }
        }
        self
    }
}

impl From<Update> for Document {
    /// Convert the builder into its update document.
    fn from(update: Update) -> Self {
        update.inner
    }
}

/// Serialize a value into a [`Bson`], mapping failures to a typed error.
fn to_bson<T: Serialize>(value: &T) -> Result<Bson> {
    bson::serialize_to_bson(value).map_err(|e| MongoError::Serialization(e.to_string()))
}
