//! Typed errors for OpenAPI document generation.
//!
//! Generation fails **loudly** on input it cannot represent faithfully rather
//! than silently omitting a route: an unknown HTTP method, a path that is not a
//! valid OpenAPI template, or a `$ref` to a component schema the caller never
//! supplied. Every variant carries a stable [`OpenApiError::code`] so callers
//! (the CLI, the dev docs route) can surface a machine-readable reason.

/// Failures raised while translating the route table into an OpenAPI document.
#[derive(Debug, thiserror::Error)]
pub enum OpenApiError {
    /// A route declared an HTTP method that OpenAPI 3.1 does not document
    /// (anything outside `get`/`put`/`post`/`delete`/`options`/`head`/`patch`/
    /// `trace`), or an empty method string.
    #[error("unsupported HTTP method `{method}`")]
    UnsupportedMethod {
        /// The offending method as declared on the route entry.
        method: String,
    },

    /// A route path could not be translated into an OpenAPI path template —
    /// it does not begin with `/`, or carries an empty/unterminated
    /// `{placeholder}`.
    #[error("invalid route path `{path}`")]
    InvalidPath {
        /// The offending path as declared on the route entry.
        path: String,
    },

    /// [`crate::schema_ref`] was asked for a component schema that is not in the
    /// caller-supplied registry, so no `$ref` can be emitted.
    #[error("unknown component schema `{name}`")]
    UnknownSchema {
        /// The requested component schema name.
        name: String,
    },
}

impl OpenApiError {
    /// Stable, machine-readable error code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedMethod { .. } => "openapi.unsupported_method",
            Self::InvalidPath { .. } => "openapi.invalid_path",
            Self::UnknownSchema { .. } => "openapi.unknown_schema",
        }
    }
}

/// Convenience alias for OpenAPI results.
pub type OpenApiResult<T> = std::result::Result<T, OpenApiError>;
