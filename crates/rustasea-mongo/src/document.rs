//! The [`Document`] contract binding a Rust type to a MongoDB collection.
//!
//! Implementors declare which collection stores them and, optionally, expose
//! their `_id` as a string. The [`crate::Collection`] wrapper uses
//! [`Document::COLLECTION`] to resolve the driver collection and
//! [`Document::id`] to build id-based queries.

/// A type persisted in a MongoDB collection.
///
/// The trait is intentionally minimal — it carries no CRUD methods so it can be
/// implemented for plain `serde` structs without pulling in an ORM. CRUD lives
/// on [`crate::Collection`], which is parameterised by `T: Document`.
///
/// ```
/// use rustasea_mongo::Document;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Debug, Serialize, Deserialize)]
/// struct User {
///     #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
///     id: Option<String>,
///     name: String,
/// }
///
/// impl Document for User {
///     const COLLECTION: &'static str = "users";
///     fn id(&self) -> Option<&str> { self.id.as_deref() }
/// }
///
/// assert_eq!(User::COLLECTION, "users");
/// ```
pub trait Document: serde::Serialize + serde::de::DeserializeOwned + Send + Sync {
    /// Collection name that stores this document type.
    const COLLECTION: &'static str;

    /// Return the document's `_id` as a string, when present.
    ///
    /// Defaults to `None`; override when the type has a string-compatible id
    /// so [`crate::Collection::find_by_id`] and friends can be used.
    fn id(&self) -> Option<&str> {
        None
    }
}
