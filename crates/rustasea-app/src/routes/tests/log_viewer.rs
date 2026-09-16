//! Dev-only log viewer surface tests — `/_logs` and `/_logs/json` (ADOPT-014).
//!
//! Positive cases prove both routes serve their payload when the app runs in
//! debug; the negative case proves a production state (`debug == false`) hides
//! both with a `404`. The JSON endpoint is fail-soft, so a request against a
//! missing log file returns `200` with an `error` field and an empty `entries`
//! array — asserted here because the route tests run with the workspace root as
//! cwd, where the resolved fixture file may not exist.
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

/// Positive: `/_logs` serves the HTML shell pointing at the JSON endpoint.
#[tokio::test]
async fn log_viewer_ui_is_served_in_debug() {
    let (status, headers, body) = call(router_with_debug(true), get("/_logs")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html")),
        "the log viewer must be served as HTML"
    );
    assert!(body.contains("/_logs/json"), "body: {body}");
}

/// Positive: `/_logs/json` serves the expected JSON shape (fail-soft).
#[tokio::test]
async fn log_viewer_json_is_served_in_debug() {
    let (status, headers, body) = call(router_with_debug(true), get("/_logs/json")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    assert!(
        parsed.get("entries").is_some_and(|e| e.is_array()),
        "body: {body}"
    );
    assert!(parsed.get("skipped").is_some(), "body: {body}");
}

/// The JSON endpoint accepts `level`, `grep`, and `limit` query parameters.
#[tokio::test]
async fn log_viewer_json_accepts_filters() {
    let (status, _, body) = call(
        router_with_debug(true),
        get("/_logs/json?level=error&grep=boom&limit=5"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    assert!(parsed["entries"].is_array(), "body: {body}");
}

/// A missing log file is a soft failure: `200` with an `error`, never a `500`.
#[tokio::test]
async fn log_viewer_json_missing_file_is_soft() {
    // An unknown channel guarantees resolution fails regardless of cwd.
    let (status, _, body) = call(
        router_with_debug(true),
        get("/_logs/json?channel=definitely-not-a-channel"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "must not 500 on a missing file");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    assert!(parsed["entries"].as_array().is_some_and(|e| e.is_empty()));
    assert!(parsed.get("error").is_some(), "body: {body}");
}

/// Negative: in production both routes are hidden behind a `404`.
#[tokio::test]
async fn log_viewer_surface_is_hidden_in_production() {
    for uri in ["/_logs", "/_logs/json"] {
        let (status, _, _) = call(router_with_debug(false), get(uri)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "GET {uri}");
    }
}
