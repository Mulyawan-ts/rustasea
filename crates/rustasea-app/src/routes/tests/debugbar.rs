//! Dev-only debugbar surface tests — `/_debugbar` and `/_debugbar/json`.
//!
//! Positive cases prove both routes serve their payload when the app runs in
//! debug; the negative case proves a production state (`debug == false`) hides
//! both with a `404`, so neither the toolbar nor the captured profiles are
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

/// Positive: `/_debugbar` serves the HTML shell pointing at the JSON endpoint.
#[tokio::test]
async fn debugbar_ui_is_served_in_debug() {
    let (status, headers, body) = call(router_with_debug(true), get("/_debugbar")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html")),
        "debugbar must be served as HTML"
    );
    assert!(body.contains("/_debugbar/json"), "body: {body}");
}

/// Positive: `/_debugbar/json` serves a JSON array (possibly empty).
#[tokio::test]
async fn debugbar_json_is_served_in_debug() {
    let (status, headers, body) = call(router_with_debug(true), get("/_debugbar/json")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON array");
    assert!(
        parsed.is_array(),
        "the payload must be a JSON array, got: {body}"
    );
}

/// Negative: in production both routes are hidden behind a `404`.
#[tokio::test]
async fn debugbar_surface_is_hidden_in_production() {
    for uri in ["/_debugbar", "/_debugbar/json"] {
        let (status, _, _) = call(router_with_debug(false), get(uri)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "GET {uri}");
    }
}
