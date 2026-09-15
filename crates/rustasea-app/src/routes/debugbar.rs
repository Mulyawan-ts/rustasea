//! Developer profiler surface — `/_debugbar` and `/_debugbar/json` (ADOPT-009).
//!
//! Both routes are **dev-only**: they are gated on [`AppState::debug`], so a
//! production build (debug `false`) answers `404` and never exposes the captured
//! profiles. The module itself is only compiled with the `debugbar` feature, so
//! a default build has neither the routes nor the profiler hooks.

use std::sync::Arc;

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

use rustasea::http::AppState;
use rustasea::router::Router as RouteTable;

/// The debug toolbar HTML shell, served at `/_debugbar`.
///
/// Kept as a sibling asset so the markup is editable without touching Rust
/// code; `include_str!` bakes it into the binary at compile time.
const DEBUGBAR_HTML: &str = include_str!("debugbar.html");

/// Register the dev-only profiler routes onto `table`.
pub fn register(table: &mut RouteTable) {
    table.get_action("/_debugbar", debugbar_ui);
    table.get_action("/_debugbar/json", debugbar_json);
}

/// GET /_debugbar — serve the debug toolbar shell (dev only).
///
/// The shell fetches `/_debugbar/json`; in production (`debug == false`) the
/// route answers `404` so neither the UI nor the captured data is reachable.
async fn debugbar_ui(Extension(state): Extension<Arc<AppState>>) -> Response {
    if !state.debug {
        return StatusCode::NOT_FOUND.into_response();
    }
    Html(DEBUGBAR_HTML).into_response()
}

/// GET /_debugbar/json — serve the captured request profiles (dev only).
///
/// Serializes the newest-last ring contents as a JSON array. In production
/// (`debug == false`) the route answers `404`. An empty ring serializes to `[]`.
async fn debugbar_json(Extension(state): Extension<Arc<AppState>>) -> Response {
    if !state.debug {
        return StatusCode::NOT_FOUND.into_response();
    }
    axum::Json(rustasea_debugbar::snapshot()).into_response()
}
