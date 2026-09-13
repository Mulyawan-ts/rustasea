//! Web routes — landing, health, welcome, and the authenticated dashboard.
//!
//! `/`, `/health`, and `/welcome` are ungated and keep their original
//! behaviour; `/dashboard` is the authenticated surface, gated by the `auth`
//! and `verified` middleware ids registered in [`super`].

use std::sync::Arc;

use axum::extract::Extension;
use axum::response::{Html, Response};

use rustasea::http::AppState;
use rustasea::router::Router as RouteTable;

use super::{AUTH, VERIFIED};

/// Welcome page markup, rendered from `resources/views/welcome.html`.
const WELCOME_HTML: &str = include_str!("../../../../resources/views/welcome.html");

/// Minimal dashboard markup served once the `auth` gate is satisfied.
const DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Dashboard</title></head>
<body><main><h1>Dashboard</h1>
<p>You are authenticated. Application data lands here.</p>
</main></body></html>"#;

/// Register the web route table onto `table`.
///
/// `/dashboard` is registered inside a [`RouteTable::group`] because
/// [`RouteTable::middleware`] is sticky — its pending middleware applies to
/// every subsequent route on the same table. A group gives the gate its own
/// scope so it cannot leak onto routes registered later (auth, settings,
/// console).
pub fn register(table: &mut RouteTable) {
    table.get_action("/", index).named("home");
    table.get_action("/health", health);
    table.get_action("/welcome", index);
    table.group(|group| {
        group
            .middleware(AUTH)
            .middleware(VERIFIED)
            .get_action("/dashboard", dashboard)
            .named("dashboard");
    });
}

/// GET / and /welcome — serve the Laravel-style welcome page.
async fn index() -> Html<&'static str> {
    Html(WELCOME_HTML)
}

/// Health check response body.
#[derive(serde::Serialize)]
struct Health {
    /// Liveness indicator.
    status: &'static str,
    /// Service name.
    service: &'static str,
    /// Current application environment.
    env: String,
}

/// GET /health — JSON liveness probe with HTTP status.
async fn health(Extension(state): Extension<Arc<AppState>>) -> Response {
    rustasea::http::JsonResponse::ok(Health {
        status: "ok",
        service: "rustasea-app",
        env: state.env.clone(),
    })
}

/// GET /dashboard — minimal authenticated page behind the `auth` gate.
async fn dashboard() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}
