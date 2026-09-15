//! Unit tests for OpenAPI generation.
//!
//! Covers the documented positive paths (path/parameters, segment-name
//! preservation, envelope shape, metadata derivation) and the fail-loud
//! negatives (unsupported method, invalid path, dangling `$ref`).

use std::collections::HashMap;

use rustasea_router::{AuthorizeSpec, RouteEntry};
use serde_json::{json, Value};

use crate::{
    generate, generate_with_components, generate_with_info, schema_ref, OpenApiError, SpecInfo,
};

/// Build a route entry with the given method/path and empty metadata.
fn entry(method: &str, path: &str) -> RouteEntry {
    RouteEntry {
        method: method.to_string(),
        path: path.to_string(),
        name: None,
        middleware: Vec::new(),
        authorizations: Vec::new(),
        domain: None,
        binding_fields: Vec::new(),
        controller: None,
        handler: None,
    }
}

/// A `{id}` placeholder yields a required string path parameter.
#[test]
fn simple_placeholder_becomes_a_path_parameter() {
    let spec = generate(&[entry("GET", "/users/{id}")]).expect("generates");
    let operation = &spec["paths"]["/users/{id}"]["get"];
    assert!(operation.is_object(), "operation missing: {spec}");
    assert_eq!(operation["parameters"][0]["name"], "id");
    assert_eq!(operation["parameters"][0]["in"], "path");
    assert_eq!(operation["parameters"][0]["required"], true);
    assert_eq!(operation["parameters"][0]["schema"]["type"], "string");
}

/// A route-model binding selector is dropped from the template: the segment
/// name (`user`) is kept, not the field name (`slug`).
#[test]
fn model_binding_selector_keeps_the_segment_name() {
    let spec = generate(&[entry("GET", "/users/{user:slug}")]).expect("generates");
    assert!(
        spec["paths"]["/users/{user}"]["get"].is_object(),
        "expected /users/{{user}} template: {spec}"
    );
    assert!(
        spec["paths"]["/users/{user:slug}"].is_null(),
        "the selector must not leak into the template: {spec}"
    );
    assert_eq!(
        spec["paths"]["/users/{user}"]["get"]["parameters"][0]["name"],
        "user"
    );
}

/// The envelope declares OpenAPI 3.1 and carries info + empty components.
#[test]
fn envelope_declares_openapi_31() {
    let spec = generate(&[]).expect("generates");
    assert_eq!(spec["openapi"], "3.1.0");
    assert_eq!(spec["info"]["title"], "RustaSea API");
    assert_eq!(spec["info"]["version"], "0.1.0");
    assert!(spec["components"]["schemas"].is_object());
    assert!(spec["paths"].is_object());
}

/// A custom [`SpecInfo`] overrides the default title/version.
#[test]
fn custom_info_overrides_the_envelope() {
    let info = SpecInfo::new("Billing API", "2.4.0");
    let spec = generate_with_info(&[], &info).expect("generates");
    assert_eq!(spec["info"]["title"], "Billing API");
    assert_eq!(spec["info"]["version"], "2.4.0");
}

/// An unknown method is a typed error, never a silently dropped route.
#[test]
fn unknown_method_is_a_typed_error() {
    let error = generate(&[entry("BREW", "/coffee")]).expect_err("must fail loud");
    assert!(
        matches!(error, OpenApiError::UnsupportedMethod { .. }),
        "expected UnsupportedMethod, got {error:?}"
    );
    assert_eq!(error.code(), "openapi.unsupported_method");
    assert!(error.to_string().contains("BREW"));
}

/// An empty method is rejected the same way as an unknown one.
#[test]
fn empty_method_is_a_typed_error() {
    let error = generate(&[entry("", "/coffee")]).expect_err("must fail loud");
    assert!(matches!(error, OpenApiError::UnsupportedMethod { .. }));
}

/// A malformed path (missing leading slash) is a typed error.
#[test]
fn path_without_leading_slash_is_a_typed_error() {
    let error = generate(&[entry("GET", "users/{id}")]).expect_err("must fail loud");
    assert!(
        matches!(error, OpenApiError::InvalidPath { .. }),
        "expected InvalidPath, got {error:?}"
    );
    assert_eq!(error.code(), "openapi.invalid_path");
}

/// An unterminated placeholder is a typed error.
#[test]
fn unterminated_placeholder_is_a_typed_error() {
    let error = generate(&[entry("GET", "/users/{id")]).expect_err("must fail loud");
    assert!(matches!(error, OpenApiError::InvalidPath { .. }));
}

/// An unmatched closing brace outside a placeholder is a typed error.
#[test]
fn stray_closing_brace_is_a_typed_error() {
    let error = generate(&[entry("GET", "/users/id}")]).expect_err("must fail loud");
    assert!(
        matches!(error, OpenApiError::InvalidPath { .. }),
        "expected InvalidPath, got {error:?}"
    );
    assert_eq!(error.code(), "openapi.invalid_path");
}

/// A `}` immediately after a well-formed placeholder is also rejected.
#[test]
fn trailing_brace_after_placeholder_is_a_typed_error() {
    let error = generate(&[entry("GET", "/users/{id}}")]).expect_err("must fail loud");
    assert!(matches!(error, OpenApiError::InvalidPath { .. }));
}

/// Valid placeholder paths still translate unchanged.
#[test]
fn valid_placeholder_paths_still_translate() {
    let spec = generate(&[
        entry("GET", "/users/{id}"),
        entry("GET", "/teams/{team}/members/{member:slug}"),
    ])
    .expect("generates");
    assert!(spec["paths"]["/users/{id}"]["get"].is_object());
    assert!(spec["paths"]["/teams/{team}/members/{member}"]["get"].is_object());
}

/// Empty routes still yield a valid, complete document.
#[test]
fn empty_route_table_is_valid() {
    let spec = generate(&[]).expect("generates");
    assert_eq!(spec["openapi"], "3.1.0");
    assert_eq!(spec["paths"].as_object().expect("paths object").len(), 0);
}

/// `operationId`, `summary`, `tags` and `x-authorizations` derive from route
/// metadata.
#[test]
fn operation_metadata_is_derived_from_the_entry() {
    let mut route = entry("POST", "/users/{user:slug}/posts");
    route.name = Some("users.posts.store".to_string());
    route.handler = Some("PostController::store".to_string());
    route.authorizations = vec![
        AuthorizeSpec::Resource {
            ability: "create".to_string(),
            resource: "user".to_string(),
        },
        AuthorizeSpec::Ability {
            name: "posts.write".to_string(),
        },
    ];

    let spec = generate(&[route]).expect("generates");
    let operation = &spec["paths"]["/users/{user}/posts"]["post"];
    assert_eq!(operation["operationId"], "users.posts.store");
    assert_eq!(operation["summary"], "PostController::store");
    assert_eq!(operation["tags"][0], "users");
    assert_eq!(operation["x-authorizations"][0], "create");
    assert_eq!(operation["x-authorizations"][1], "posts.write");
    assert!(
        operation["security"].is_null(),
        "no invalid `security` array must be emitted: {spec}"
    );
    assert_eq!(operation["parameters"][0]["name"], "user");
}

/// A later entry wins for a repeated `(path, method)` slot.
#[test]
fn later_entry_wins_for_a_duplicate_slot() {
    let mut first = entry("GET", "/users");
    first.name = Some("first".to_string());
    let mut second = entry("GET", "/users");
    second.name = Some("second".to_string());

    let spec = generate(&[first, second]).expect("generates");
    assert_eq!(spec["paths"]["/users"]["get"]["operationId"], "second");
}

/// Caller-supplied component schemas are copied into `components.schemas`.
#[test]
fn component_schemas_are_injected() {
    let mut components = HashMap::new();
    components.insert("User".to_string(), json!({ "type": "object" }));

    let spec = generate_with_components(&[], &SpecInfo::default(), &components).expect("generates");
    assert_eq!(spec["components"]["schemas"]["User"]["type"], "object");
}

/// `schema_ref` resolves a registered schema and refuses an unknown one.
#[test]
fn schema_ref_resolves_or_fails_loud() {
    let mut components = HashMap::new();
    components.insert("User".to_string(), json!({ "type": "object" }));

    let reference = schema_ref(&components, "User").expect("registered");
    assert_eq!(reference["$ref"], "#/components/schemas/User");

    let error = schema_ref(&components, "Missing").expect_err("must fail loud");
    assert!(matches!(error, OpenApiError::UnknownSchema { .. }));
    assert_eq!(error.code(), "openapi.unknown_schema");
}

/// `head` and `options` are valid operation keys; a multi-parameter path keeps
/// its parameters in order and de-duplicates repeated names.
#[test]
fn head_options_and_duplicate_params() {
    let spec = generate(&[
        entry("HEAD", "/health"),
        entry("OPTIONS", "/teams/{team}/members/{member:slug}"),
    ])
    .expect("generates");
    assert!(spec["paths"]["/health"]["head"].is_object());
    let params = spec["paths"]["/teams/{team}/members/{member}"]["options"]["parameters"]
        .as_array()
        .expect("parameters array");
    assert_eq!(params.len(), 2);
    assert_eq!(params[0]["name"], "team");
    assert_eq!(params[1]["name"], "member");
}

/// Guard against the JSON value never being a non-object document.
#[test]
fn document_root_is_an_object() {
    let spec: Value = generate(&[]).expect("generates");
    assert!(spec.is_object());
}
