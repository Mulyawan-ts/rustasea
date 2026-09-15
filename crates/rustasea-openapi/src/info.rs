//! OpenAPI document envelope metadata.
//!
//! The `info` object of an OpenAPI document carries the human-facing title and
//! version. [`SpecInfo`] is the caller-supplied pair; [`OPENAPI_VERSION`] is the
//! fixed specification version this crate emits.

/// The OpenAPI specification version every generated document declares.
pub const OPENAPI_VERSION: &str = "3.1.0";

/// Metadata rendered into the document's `info` object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecInfo {
    /// Human-readable API title (`info.title`).
    pub title: String,
    /// API version string (`info.version`).
    pub version: String,
}

impl SpecInfo {
    /// Create envelope metadata from a title and version.
    pub fn new(title: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            version: version.into(),
        }
    }
}

impl Default for SpecInfo {
    /// The framework default envelope: `RustaSea API` at the workspace version.
    fn default() -> Self {
        Self::new("RustaSea API", "0.1.0")
    }
}
