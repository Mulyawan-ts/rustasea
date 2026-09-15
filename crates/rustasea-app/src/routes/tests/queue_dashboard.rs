//! Queue dashboard surface tests (ADOPT-021).
//!
//! Positive: with the surface enabled and the app in debug, `/queue` serves the
//! HTML shell and `/queue/metrics.json` / `/queue/failed.json` serve JSON; an
//! authenticated retry re-enqueues the job and an authenticated forget deletes
//! the row. Negative: a production state (`debug == false`) and a disabled
//! config both answer `404`; an unauthenticated request is redirected to
//! `/login`; a `POST` retry without the CSRF token is rejected `403`.
//!
//! The dashboard crate's config and pool are process-wide, so every test
//! serializes through [`DASHBOARD_LOCK`] and installs a fresh in-memory pool.
//!
//! Gated on the `queue-dashboard` feature because the routes/module only exist
//! with it.

use std::sync::Arc;

use axum::extract::Extension;
use axum::http::{header, StatusCode};
use axum::Router;
use rustasea::auth::AuthUser;
use rustasea::http::AppState;
use rustasea::orm::DbPool;
use rustasea_queue_dashboard::{DashboardConfig, JobId};

use super::{call, csrf_same_origin, get, method_request};
use crate::routes::{compile, table};

/// Serializes tests that touch the process-wide dashboard config/pool.
static DASHBOARD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Build a migrated in-memory pool with `jobs` + `failed_jobs` + `queue_metrics`.
async fn pool() -> DbPool {
    let pool = DbPool::connect("sqlite::memory:")
        .await
        .expect("in-memory pool");
    pool.execute_script(
        "CREATE TABLE jobs (\
         id TEXT PRIMARY KEY, queue TEXT NOT NULL, payload TEXT NOT NULL, \
         attempts INTEGER NOT NULL DEFAULT 0, reserved_at TEXT, \
         available_at TEXT NOT NULL, created_at TEXT NOT NULL); \
         CREATE TABLE failed_jobs (\
         id TEXT PRIMARY KEY, connection TEXT NOT NULL, queue TEXT NOT NULL, \
         payload TEXT NOT NULL, exception TEXT NOT NULL, failed_at TEXT NOT NULL); \
         CREATE TABLE queue_metrics (\
         sampled_at TEXT NOT NULL, connection TEXT NOT NULL, queue TEXT NOT NULL, \
         pending INTEGER NOT NULL, delayed INTEGER NOT NULL, reserved INTEGER NOT NULL, \
         oldest_pending TEXT, PRIMARY KEY (sampled_at, connection, queue))",
    )
    .await
    .expect("schema");
    pool
}

/// Install the enabled config + the pool into the process-wide slots.
fn install(pool: &DbPool) {
    rustasea_queue_dashboard::set_config(DashboardConfig {
        enabled: true,
        retention_minutes: 60,
        sample_interval_secs: 60,
    });
    rustasea_queue_dashboard::set_pool(pool.clone());
}

/// A router compiled with the given debug flag.
fn router_with_debug(debug: bool) -> Router {
    compile(table(), Arc::new(AppState::new("testing", debug)))
}

/// A router with an authenticated, verified principal.
fn authed_router() -> Router {
    authed_router_with_debug(true)
}

/// A router with an authenticated, verified principal and the given debug flag.
fn authed_router_with_debug(debug: bool) -> Router {
    let principal = AuthUser::new("user-1", Some("ada@example.com"), "session")
        .with_email_verified_at(Some("2026-01-01T00:00:00Z"));
    router_with_debug(debug).layer(Extension(principal))
}

/// Insert a dead-letter row and return its id.
async fn seed_failed(pool: &DbPool) -> JobId {
    let id = JobId::new();
    let payload = serde_json::json!({
        "queue": "default", "connection": "database", "available_at": null,
        "attempts": 1, "id": null, "job": "test::Failed", "batch_id": null, "payload": {}
    });
    pool.execute_bind(
        "INSERT INTO failed_jobs (id, connection, queue, payload, exception, failed_at) \
         VALUES ($1, $2, $3, $4, $5, $6)",
        &[
            rustasea::orm::Value::Text(id.to_string()),
            rustasea::orm::Value::Text("database".to_string()),
            rustasea::orm::Value::Text("default".to_string()),
            rustasea::orm::Value::Text(payload.to_string()),
            rustasea::orm::Value::Text("boom".to_string()),
            rustasea::orm::Value::Text(
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            ),
        ],
    )
    .await
    .expect("seed");
    id
}

/// Count rows in `failed_jobs`.
async fn failed_count(pool: &DbPool) -> usize {
    pool.fetch_json("SELECT COUNT(*) AS c FROM failed_jobs", &[])
        .await
        .expect("count")
        .first()
        .and_then(|r| r.get("c"))
        .and_then(|v| v.as_i64())
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(0)
}

/// Positive: the enabled surface serves the HTML shell and both JSON endpoints.
#[tokio::test]
async fn enabled_surface_serves_ui_and_json() {
    let _guard = DASHBOARD_LOCK.lock().await;
    let pool = pool().await;
    install(&pool);

    let (status, headers, body) = call(authed_router(), get("/queue")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html")));
    assert!(body.contains("/queue/metrics.json"), "body: {body}");
    assert!(
        !body.contains("__CSRF_TOKEN__"),
        "the token must be injected"
    );

    let (status, _, body) = call(authed_router(), get("/queue/metrics.json")).await;
    assert_eq!(status, StatusCode::OK);
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("metrics JSON");
    assert!(parsed.get("snapshot").is_some());
    assert!(parsed.get("history").is_some());
    assert!(parsed.get("workers").is_some());

    let (status, _, body) = call(authed_router(), get("/queue/failed.json")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(serde_json::from_str::<serde_json::Value>(&body)
        .expect("failed JSON")
        .is_array());

    rustasea_queue_dashboard::clear_pool();
}

/// Negative: a production state (`debug == false`) hides the whole surface.
#[tokio::test]
async fn production_state_hides_surface() {
    let _guard = DASHBOARD_LOCK.lock().await;
    let pool = pool().await;
    install(&pool);

    for uri in ["/queue", "/queue/metrics.json", "/queue/failed.json"] {
        let (status, _, _) = call(authed_router_with_debug(false), get(uri)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "GET {uri}");
    }
    rustasea_queue_dashboard::clear_pool();
}

/// Negative: a disabled config hides the surface even in debug.
#[tokio::test]
async fn disabled_config_hides_surface() {
    let _guard = DASHBOARD_LOCK.lock().await;
    rustasea_queue_dashboard::set_config(DashboardConfig::default());
    let (status, _, _) = call(authed_router(), get("/queue")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Negative: an unauthenticated request is redirected to `/login`.
#[tokio::test]
async fn unauthenticated_is_redirected_to_login() {
    let _guard = DASHBOARD_LOCK.lock().await;
    let pool = pool().await;
    install(&pool);

    let (status, headers, _) = call(router_with_debug(true), get("/queue")).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/login")
    );
    rustasea_queue_dashboard::clear_pool();
}

/// Negative: a `POST` retry without the CSRF token is rejected `403`.
#[tokio::test]
async fn retry_without_csrf_is_rejected() {
    let _guard = DASHBOARD_LOCK.lock().await;
    let pool = pool().await;
    install(&pool);
    let id = seed_failed(&pool).await;

    let uri = format!("/queue/failed/{id}/retry");
    let (status, _, _) = call(authed_router(), method_request("POST", &uri)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(failed_count(&pool).await, 1, "the row is untouched");
    rustasea_queue_dashboard::clear_pool();
}

/// Positive: an authenticated retry re-enqueues the job and deletes the row.
#[tokio::test]
async fn retry_reenqueues_and_deletes() {
    let _guard = DASHBOARD_LOCK.lock().await;
    let pool = pool().await;
    install(&pool);
    let id = seed_failed(&pool).await;

    let uri = format!("/queue/failed/{id}/retry");
    let request = csrf_same_origin(method_request("POST", &uri));
    let (status, headers, _) = call(authed_router(), request).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/queue")
    );
    assert_eq!(failed_count(&pool).await, 0, "the row is deleted");
    let jobs = pool
        .fetch_json("SELECT COUNT(*) AS c FROM jobs", &[])
        .await
        .expect("count jobs")
        .first()
        .and_then(|r| r.get("c"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    assert_eq!(jobs, 1, "the job is re-enqueued");
    rustasea_queue_dashboard::clear_pool();
}

/// Positive: an authenticated forget deletes the row.
#[tokio::test]
async fn forget_deletes_the_row() {
    let _guard = DASHBOARD_LOCK.lock().await;
    let pool = pool().await;
    install(&pool);
    let id = seed_failed(&pool).await;

    let uri = format!("/queue/failed/{id}");
    let request = csrf_same_origin(method_request("DELETE", &uri));
    let (status, _, _) = call(authed_router(), request).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(failed_count(&pool).await, 0);
    rustasea_queue_dashboard::clear_pool();
}

/// Negative: forgetting an unknown id answers `404`.
#[tokio::test]
async fn forget_unknown_id_is_404() {
    let _guard = DASHBOARD_LOCK.lock().await;
    let pool = pool().await;
    install(&pool);

    let uri = format!("/queue/failed/{}", JobId::new());
    let request = csrf_same_origin(method_request("DELETE", &uri));
    let (status, _, _) = call(authed_router(), request).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    rustasea_queue_dashboard::clear_pool();
}
