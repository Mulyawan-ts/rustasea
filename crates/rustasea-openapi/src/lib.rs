//! OpenAPI 3.1 document generation from the live route table.
//!
//! This crate is the RustaSea analogue of `dedoc/scramble`: it turns the
//! introspectable [`RouteEntry`] metadata the router already accumulates into a
//! valid OpenAPI 3.1 document. The route table is the single source of truth —
//! the same registry that backs `route:list` feeds the spec, so the documented
//! surface and the served surface cannot diverge.
//!
//! # Scope
//!
//! Generation is intentionally metadata-driven: paths, path parameters,
//! operation ids, summaries, tags and authorization markers are derived from the
//! route entries. Request/response **schemas** are not inferred by reflection
//! (Rust has none); callers that derive schemas elsewhere hand them in through
//! [`generate_with_components`] as a plain `{name: schema}` registry, and can
//! reference them with [`schema_ref`].
//!
//! # Fail loud
//!
//! Input that cannot be represented faithfully is a typed [`OpenApiError`], not
//! a silent omission: an unknown HTTP method ([`OpenApiError::UnsupportedMethod`]),
//! a malformed path ([`OpenApiError::InvalidPath`]), or a `$ref` to an
//! unregistered schema ([`OpenApiError::UnknownSchema`]).
//!
//! # Example
//!
//! ```rust
//! use rustasea_router::RouteEntry;
//! use rustasea_openapi::{generate, SpecInfo};
//!
//! let entry = RouteEntry {
//!     method: "GET".into(),
//!     path: "/users/{id}".into(),
//!     name: Some("users.show".into()),
//!     middleware: Vec::new(),
//!     authorizations: Vec::new(),
//!     domain: None,
//!     binding_fields: vec!["id".into()],
//!     controller: None,
//!     handler: Some("UserController::show".into()),
//! };
//! let spec = generate(&[entry]).expect("valid route table");
//! assert_eq!(spec["openapi"], "3.1.0");
//! assert!(spec["paths"]["/users/{id}"]["get"].is_object());
//! # let _ = SpecInfo::default();
//! ```

pub mod error;
pub mod info;
mod paths;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use rustasea_router::RouteEntry;
use serde_json::{json, Map, Value};

pub use error::{OpenApiError, OpenApiResult};
pub use info::{SpecInfo, OPENAPI_VERSION};

/// Generate an OpenAPI 3.1 document with the default [`SpecInfo`] envelope and
/// no component schemas.
///
/// Shorthand for [`generate_with_components`] with empty components; the common
/// case for a caller that only needs the documented path surface.
pub fn generate(entries: &[RouteEntry]) -> OpenApiResult<Value> {
    generate_with_info(entries, &SpecInfo::default())
}

/// Generate an OpenAPI 3.1 document with a caller-supplied [`SpecInfo`] and no
/// component schemas.
pub fn generate_with_info(entries: &[RouteEntry], info: &SpecInfo) -> OpenApiResult<Value> {
    generate_with_components(entries, info, &HashMap::new())
}

/// Generate a full OpenAPI 3.1 document.
///
/// `components` is a caller-supplied `{name: schema}` registry copied verbatim
/// into `components.schemas`; it is the schema-injection seam that keeps this
/// crate free of any schema-derive dependency. [`schema_ref`] builds the `$ref`
/// strings that point back into this registry.
///
/// # Errors
///
/// Returns a typed [`OpenApiError`] when any route entry declares an
/// unsupported method ([`OpenApiError::UnsupportedMethod`]) or an invalid path
/// ([`OpenApiError::InvalidPath`]) — never a silently dropped route.
pub fn generate_with_components(
    entries: &[RouteEntry],
    info: &SpecInfo,
    components: &HashMap<String, Value>,
) -> OpenApiResult<Value> {
    let paths = paths::build_paths(entries)?;
    let schemas: Map<String, Value> = components
        .iter()
        .map(|(name, schema)| (name.clone(), schema.clone()))
        .collect();

    Ok(json!({
        "openapi": OPENAPI_VERSION,
        "info": {
            "title": info.title,
            "version": info.version,
        },
        "paths": paths,
        "components": { "schemas": schemas },
    }))
}

/// Build a JSON Schema `$ref` into the component-schema registry.
///
/// Returns `{"$ref": "#/components/schemas/<name>"}` when `name` is registered,
/// or [`OpenApiError::UnknownSchema`] when it is not — a dangling reference is
/// refused rather than emitted.
pub fn schema_ref(components: &HashMap<String, Value>, name: &str) -> OpenApiResult<Value> {
    if components.contains_key(name) {
        Ok(json!({ "$ref": format!("#/components/schemas/{name}") }))
    } else {
        Err(OpenApiError::UnknownSchema {
            name: name.to_string(),
        })
    }
}
