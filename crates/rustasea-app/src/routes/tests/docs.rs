//! Dev-only docs surface tests — `/openapi.json` and `/docs`.
//!
//! Positive cases prove both routes serve their payload when the app runs in
//! debug; the negative case proves a production state (`debug == false`) hides
//! both with a `404`, so the specification and the CDN UI bundle are never
//! exposed in production.
//!
//! Split into a sibling module so `tests.rs` stays under the 500-line cap.

use std::sync::Arc;

use axum::http::StatusCode;
use rustasea::http::AppState;

use super::{call, get};
use crate::routes::{compile, table};

/// A router compiled with the given debug flag.
fn router_with_debug(debug: bool) -> axum::Router {
    compile(table(), Arc::new(AppState::new("testing", debug)))
}

/// Positive: `/openapi.json` serves a valid OpenAPI 3.1 document in dev.
#[tokio::test]
async fn openapi_json_is_served_in_debug() {
    let (status, headers, body) = call(router_with_debug(true), get("/openapi.json")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON spec");
    assert_eq!(parsed["openapi"], "3.1.0");
    assert!(parsed["paths"]["/health"]["get"].is_object());
    assert!(parsed["paths"]["/openapi.json"]["get"].is_object());
}

/// Positive: `/docs` serves the Scalar shell pointing at the spec.
#[tokio::test]
async fn docs_ui_is_served_in_debug() {
    let (status, headers, body) = call(router_with_debug(true), get("/docs")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html")),
        "docs must be served as HTML"
    );
    assert!(body.contains("data-url=\"/openapi.json\""), "body: {body}");
    assert!(body.contains("@scalar/api-reference"), "body: {body}");
}

/// Negative: in production both routes are hidden behind a `404`.
#[tokio::test]
async fn docs_surface_is_hidden_in_production() {
    for uri in ["/openapi.json", "/docs"] {
        let (status, _, body) = call(router_with_debug(false), get(uri)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "GET {uri}");
        assert!(!body.contains("3.1.0"), "GET {uri} must not leak the spec");
    }
}
