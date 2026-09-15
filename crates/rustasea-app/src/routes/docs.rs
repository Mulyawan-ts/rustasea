//! Developer API-docs surface — `/openapi.json` and the `/docs` Scalar UI.
//!
//! Both routes are **dev-only**: they are gated on [`AppState::debug`], so a
//! production build (debug `false`) answers `404` and never exposes the
//! generated specification or the third-party UI bundle. The spec is generated
//! from the same route table the application serves, so the docs cannot drift
//! from the live surface.

use std::sync::Arc;

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

use rustasea::http::AppState;
use rustasea::router::Router as RouteTable;

/// The Scalar API-reference HTML shell, served at `/docs`.
///
/// Kept as a sibling asset so the markup is editable without touching Rust
/// code; `include_str!` bakes it into the binary at compile time.
const DOCS_HTML: &str = include_str!("docs.html");

/// Register the dev-only docs routes onto `table`.
pub fn register(table: &mut RouteTable) {
    table.get_action("/openapi.json", openapi_json);
    table.get_action("/docs", docs_ui);
}

/// GET /openapi.json — serve the generated OpenAPI 3.1 document (dev only).
///
/// Reads the live route table from [`crate::routes::table`] and renders it
/// through [`rustasea_openapi::generate`]. In production (`debug == false`) the
/// route answers `404` so the specification is never exposed. A table that
/// cannot be represented surfaces `500` rather than a partial document.
async fn openapi_json(Extension(state): Extension<Arc<AppState>>) -> Response {
    if !state.debug {
        return StatusCode::NOT_FOUND.into_response();
    }
    let entries = crate::routes::table().get_routes();
    match rustasea_openapi::generate(&entries) {
        Ok(spec) => axum::Json(spec).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{}: {error}", error.code()),
        )
            .into_response(),
    }
}

/// GET /docs — serve the Scalar API-reference shell (dev only).
///
/// The shell loads the Scalar CDN bundle and points it at `/openapi.json`; in
/// production (`debug == false`) the route answers `404` so neither the UI nor
/// the specification is reachable.
async fn docs_ui(Extension(state): Extension<Arc<AppState>>) -> Response {
    if !state.debug {
        return StatusCode::NOT_FOUND.into_response();
    }
    Html(DOCS_HTML).into_response()
}
