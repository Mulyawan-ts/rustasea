//! Queue dashboard surface — `/queue`, `/queue/metrics.json`, `/queue/failed.json`
//! and the failed-job retry/forget actions (ADOPT-021).
//!
//! Parity target: `laravel/horizon`'s read-only dashboard + failed-job management.
//! Every route is gated twice:
//!
//! * **Auth** — the whole group carries the `auth` + `verified` middleware, so an
//!   unauthenticated request is redirected to `/login` (never reaching a handler).
//! * **Config + debug** — the handlers answer `404` unless
//!   `[queue.dashboard].enabled` is set *and* [`AppState::debug`] is on, so a
//!   production process exposes nothing.
//!
//! The mutating `POST /queue/failed/{id}/retry` and `DELETE /queue/failed/{id}`
//! routes are covered by the global CSRF allow-list (see
//! [`crate::routes::helpers::csrf_protected`]). The module is only compiled with
//! the `queue-dashboard` feature.

use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

use rustasea::http::AppState;
use rustasea::router::Router as RouteTable;
use rustasea_queue_dashboard::JobId;

use crate::routes::helpers::{json_error, see_other};
use crate::routes::{AUTH, VERIFIED};

/// The dashboard HTML shell, served at `/queue`.
const QUEUE_DASHBOARD_HTML: &str = include_str!("queue_dashboard.html");

/// Register the queue-dashboard routes onto `table`.
///
/// The whole surface is grouped behind the `auth` + `verified` middleware; the
/// per-handler config/debug gate answers `404` for a disabled or production
/// deployment.
pub fn register(table: &mut RouteTable) {
    table.group(|group| {
        group
            .middleware(AUTH)
            .middleware(VERIFIED)
            .get_action("/queue", ui)
            .get_action("/queue/metrics.json", metrics_json)
            .get_action("/queue/failed.json", failed_json)
            .post_action("/queue/failed/{id}/retry", retry)
            .delete_action("/queue/failed/{id}", forget);
    });
}

/// Whether the surface is reachable: config enabled and the app in debug.
fn surface_available(state: &AppState) -> bool {
    state.debug && rustasea_queue_dashboard::enabled()
}

/// The `503` response returned when the app installed no shared pool.
fn unavailable() -> Response {
    json_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "QueueDashboardUnavailable",
        "the queue dashboard has no database pool installed",
    )
}

/// GET /queue — serve the dashboard shell (dev + config gated).
///
/// The process-wide CSRF token is substituted into the `__CSRF_TOKEN__`
/// placeholder so the page's vanilla-JS `fetch` calls can send it as the
/// `x-csrf-token` header the global CSRF gate reads (a plain HTML form cannot
/// set a custom header). The token is HTML-escaped before substitution.
async fn ui(Extension(state): Extension<Arc<AppState>>) -> Response {
    if !surface_available(&state) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let token = html_escape(crate::routes::helpers::csrf_token());
    let html = QUEUE_DASHBOARD_HTML.replace("__CSRF_TOKEN__", &token);
    Html(html).into_response()
}

/// Escape the characters that are unsafe in an HTML text/attribute context.
///
/// The CSRF token is generated from hex digits, so this is belt-and-braces: an
/// operator-pinned `RUSTASEA_CSRF_TOKEN` could contain arbitrary bytes.
fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// GET /queue/metrics.json — live snapshot, history window, and worker heartbeats.
async fn metrics_json(Extension(state): Extension<Arc<AppState>>) -> Response {
    if !surface_available(&state) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(pool) = rustasea_queue_dashboard::pool() else {
        return unavailable();
    };
    let config = rustasea_queue_dashboard::config();
    let since =
        chrono::Utc::now() - chrono::Duration::from_std(config.retention()).unwrap_or_default();
    let snapshot = match rustasea_queue_dashboard::DashboardData::snapshot().await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "QueueMetricsFailed",
                &error.to_string(),
            )
        }
    };
    let history = match rustasea_queue_dashboard::DashboardData::history(&pool, since).await {
        Ok(history) => history,
        Err(error) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "QueueHistoryFailed",
                &error.to_string(),
            )
        }
    };
    let payload = rustasea_queue_dashboard::MetricsResponse {
        snapshot,
        history,
        workers: rustasea_queue_dashboard::DashboardData::workers(),
    };
    axum::Json(payload).into_response()
}

/// GET /queue/failed.json — the persisted failed-job list, oldest first.
async fn failed_json(Extension(state): Extension<Arc<AppState>>) -> Response {
    if !surface_available(&state) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(pool) = rustasea_queue_dashboard::pool() else {
        return unavailable();
    };
    match rustasea_queue_dashboard::DashboardData::failed_jobs(&pool).await {
        Ok(jobs) => axum::Json(jobs).into_response(),
        Err(error) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "QueueFailedJobsFailed",
            &error.to_string(),
        ),
    }
}

/// POST /queue/failed/{id}/retry — re-enqueue a dead-lettered job.
///
/// A browser form post redirects back to `/queue`; a non-browser client without
/// a `Referer` gets a JSON envelope. An unknown id is a `404`.
async fn retry(Extension(state): Extension<Arc<AppState>>, Path(id): Path<String>) -> Response {
    if !surface_available(&state) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(pool) = rustasea_queue_dashboard::pool() else {
        return unavailable();
    };
    let Some(job_id) = parse_job_id(&id) else {
        return json_error(
            StatusCode::BAD_REQUEST,
            "InvalidJobId",
            "the failed-job id is not a valid UUID",
        );
    };
    match rustasea_queue_dashboard::DashboardData::retry_failed(&pool, job_id).await {
        Ok(()) => see_other("/queue"),
        Err(error) => json_error(
            StatusCode::NOT_FOUND,
            "FailedJobNotFound",
            &error.to_string(),
        ),
    }
}

/// DELETE /queue/failed/{id} — permanently forget a dead-lettered job.
async fn forget(Extension(state): Extension<Arc<AppState>>, Path(id): Path<String>) -> Response {
    if !surface_available(&state) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(pool) = rustasea_queue_dashboard::pool() else {
        return unavailable();
    };
    let Some(job_id) = parse_job_id(&id) else {
        return json_error(
            StatusCode::BAD_REQUEST,
            "InvalidJobId",
            "the failed-job id is not a valid UUID",
        );
    };
    match rustasea_queue_dashboard::DashboardData::forget_failed(&pool, job_id).await {
        Ok(true) => see_other("/queue"),
        Ok(false) => json_error(
            StatusCode::NOT_FOUND,
            "FailedJobNotFound",
            "no failed job matched that id",
        ),
        Err(error) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "QueueForgetFailed",
            &error.to_string(),
        ),
    }
}

/// Parse a failed-job id string into a [`JobId`], or `None` when malformed.
fn parse_job_id(value: &str) -> Option<JobId> {
    uuid::Uuid::parse_str(value).ok().map(JobId::from_uuid)
}
