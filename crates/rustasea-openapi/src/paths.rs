//! Route-table → OpenAPI path/operation translation.
//!
//! Each [`RouteEntry`] becomes one operation object under its path item. The
//! path template keeps the **segment name** before any model-binding selector
//! (`{user:slug}` → `/users/{user}`), mirroring the router's own
//! `to_axum_path` translation so the documented parameter names match the
//! extracted ones. Path parameters, operation ids, summaries, tags and
//! authorization markers are derived purely from the entry metadata.

use rustasea_router::{AuthorizeSpec, RouteEntry};
use serde_json::{json, Map, Value};

use crate::error::{OpenApiError, OpenApiResult};

/// HTTP methods OpenAPI 3.1 defines an operation key for.
const SUPPORTED_METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// Build the `paths` object from a route table.
///
/// Later entries win for a repeated `(path, method)` pair, so the last
/// registration of a slot is the one documented — deterministic for a given
/// input order (the router itself sorts domain routes first).
///
/// # Errors
///
/// Propagates [`OpenApiError::UnsupportedMethod`] / [`OpenApiError::InvalidPath`]
/// from [`translate_path`] / [`operation_method`].
pub(crate) fn build_paths(entries: &[RouteEntry]) -> OpenApiResult<Value> {
    let mut paths: Map<String, Value> = Map::new();
    for entry in entries {
        let (template, params) = translate_path(&entry.path)?;
        let method = operation_method(&entry.method)?;
        let operation = build_operation(entry, &params);
        let path_item = paths
            .entry(template)
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(operations) = path_item {
            operations.insert(method, operation);
        }
    }
    Ok(Value::Object(paths))
}

/// Translate a Laravel-style route path into an OpenAPI path template.
///
/// Returns the template plus the ordered, de-duplicated path-parameter names.
/// Placeholders keep their segment name: `{id}` → `{id}` and the route-model
/// bound `{user:slug}` → `{user}` (the `:slug` selector is a Laravel-only
/// binding hint). Paths already in `{name}` form pass through unchanged.
///
/// # Errors
///
/// [`OpenApiError::InvalidPath`] when the path does not start with `/`, or a
/// placeholder is empty (`{}`, `{:field}`) or unterminated (`{id`), or a `}`
/// appears outside a placeholder (`/users/id}`).
pub(crate) fn translate_path(path: &str) -> OpenApiResult<(String, Vec<String>)> {
    if !path.starts_with('/') {
        return Err(OpenApiError::InvalidPath {
            path: path.to_string(),
        });
    }

    let mut template = String::with_capacity(path.len());
    let mut params: Vec<String> = Vec::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        // Any `}` in the literal run before an opening brace is unmatched.
        if rest[..open].contains('}') {
            return Err(OpenApiError::InvalidPath {
                path: path.to_string(),
            });
        }
        template.push_str(&rest[..open]);
        let tail = &rest[open + 1..];
        let Some(close) = tail.find('}') else {
            // Unterminated placeholder — refuse rather than emit a broken path.
            return Err(OpenApiError::InvalidPath {
                path: path.to_string(),
            });
        };
        let inner = &tail[..close];
        let name = inner.split(':').next().unwrap_or(inner);
        if name.is_empty() {
            return Err(OpenApiError::InvalidPath {
                path: path.to_string(),
            });
        }
        template.push('{');
        template.push_str(name);
        template.push('}');
        if !params.iter().any(|existing| existing == name) {
            params.push(name.to_string());
        }
        rest = &tail[close + 1..];
    }
    // The trailing run (after the last placeholder) must not contain a stray `}`.
    if rest.contains('}') {
        return Err(OpenApiError::InvalidPath {
            path: path.to_string(),
        });
    }
    template.push_str(rest);
    Ok((template, params))
}

/// Map a route method to its lowercase OpenAPI operation key.
///
/// # Errors
///
/// [`OpenApiError::UnsupportedMethod`] when the method (case-insensitively) is
/// not one of the eight OpenAPI operation keys, or is empty.
pub(crate) fn operation_method(method: &str) -> OpenApiResult<String> {
    let lower = method.to_ascii_lowercase();
    if SUPPORTED_METHODS.contains(&lower.as_str()) {
        Ok(lower)
    } else {
        Err(OpenApiError::UnsupportedMethod {
            method: method.to_string(),
        })
    }
}

/// Build the operation object for one route entry.
fn build_operation(entry: &RouteEntry, params: &[String]) -> Value {
    let mut operation = Map::new();

    if let Some(name) = entry.name.as_deref().filter(|n| !n.is_empty()) {
        operation.insert("operationId".to_string(), Value::String(name.to_string()));
    }
    if let Some(handler) = entry.handler.as_deref().filter(|h| !h.is_empty()) {
        operation.insert("summary".to_string(), Value::String(handler.to_string()));
    }
    if let Some(tag) = first_tag(&entry.path) {
        operation.insert("tags".to_string(), json!([tag]));
    }
    if !params.is_empty() {
        let parameters: Vec<Value> = params.iter().map(|name| path_parameter(name)).collect();
        operation.insert("parameters".to_string(), Value::Array(parameters));
    }
    if let Some(authorizations) = authorizations_marker(&entry.authorizations) {
        operation.insert("x-authorizations".to_string(), authorizations);
    }

    // An Operation Object requires `responses`; the router does not model
    // response types, so a single generic success response is documented.
    operation.insert(
        "responses".to_string(),
        json!({ "200": { "description": "Successful response" } }),
    );

    Value::Object(operation)
}

/// Build one path-parameter object (always a required string path segment).
fn path_parameter(name: &str) -> Value {
    json!({
        "name": name,
        "in": "path",
        "required": true,
        "schema": { "type": "string" },
    })
}

/// The first non-parameter path segment, used as a grouping tag.
///
/// `/users/{id}` → `users`; `/` or `/{id}` → `None` (no meaningful group).
fn first_tag(path: &str) -> Option<String> {
    path.split('/')
        .find(|segment| !segment.is_empty() && !segment.starts_with('{'))
        .map(str::to_string)
}

/// Build the `x-authorizations` extension for a route's `#[authorize]`
/// declarations.
///
/// OpenAPI 3.1 §4.8.27 requires every `security` requirement name to resolve to
/// a declared `components.securitySchemes` entry, and forbids non-empty scope
/// arrays on non-OAuth schemes; the router models record-level abilities rather
/// than a security scheme, so emitting `security` here would be rejected by
/// strict validators. A vendor extension keeps the intent while staying
/// spec-compliant. `None` when the route declares no authorizations.
fn authorizations_marker(authorizations: &[AuthorizeSpec]) -> Option<Value> {
    let abilities: Vec<String> = authorizations
        .iter()
        .map(|spec| match spec {
            AuthorizeSpec::Resource { ability, .. } => ability.clone(),
            AuthorizeSpec::Ability { name } => name.clone(),
        })
        .collect();
    if abilities.is_empty() {
        None
    } else {
        Some(json!(abilities))
    }
}
