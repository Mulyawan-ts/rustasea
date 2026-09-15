//! Axum `from_fn` middleware that profiles one request (ADOPT-009).
//!
//! [`profiler_middleware`] builds a [`ProfileContext`] at request entry, installs
//! it in the process-wide slot, runs the inner service, then finalizes a
//! [`RequestProfile`] into the ring buffer and clears the slot. The SQL, cache,
//! and event hooks append to the context while the inner service runs.
//!
//! # Wiring
//!
//! `from_fn` fixes its handler's state to `()`, so the debug flag is captured by
//! the caller's closure:
//!
//! ```rust,ignore
//! let debug = state.debug;
//! router.layer(axum::middleware::from_fn(move |req, next| {
//!     profiler_middleware(debug, req, next)
//! }))
//! ```
//!
//! When `debug` is `false` the middleware is a no-op: it forwards the request
//! without building a context or touching the ring, so production pays nothing.

use std::sync::Arc;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

use crate::context::{self, ProfileContext, RequestProfile};
use crate::ring;

/// Request-id header read by the middleware.
const REQUEST_ID_HEADER: &str = "x-request-id";

/// Profile a request when `debug` is `true`; otherwise pass it through.
pub async fn profiler_middleware(debug: bool, request: Request, next: Next) -> Response {
    if !debug {
        return next.run(request).await;
    }

    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let request_id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    let ctx = Arc::new(ProfileContext::new(method, path, request_id));
    context::set_current(Arc::clone(&ctx));

    let response = next.run(request).await;
    let status = response.status().as_u16();

    let (queries, cache_ops, events) = ctx.drain();
    let profile = RequestProfile {
        method: ctx.method.clone(),
        path: ctx.path.clone(),
        request_id: ctx.request_id.clone(),
        status,
        duration_ms: ctx.started.elapsed().as_secs_f64() * 1000.0,
        queries,
        cache_ops,
        events,
        started_at: chrono::Utc::now().to_rfc3339(),
    };
    ring::record(profile);
    context::clear_current_if(&ctx);

    response
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    use super::*;
    use crate::context::SqlEntry;

    /// The crate-wide serialization lock (shared with the lib/ring tests).
    static PROFILER_LOCK: &tokio::sync::Mutex<()> = &crate::test_support::PROFILER_LOCK;

    /// Build a router whose handler pushes one SQL entry into the active
    /// context, then returns `200`.
    fn router_recording_one_query() -> Router {
        Router::new()
            .route(
                "/profile-me",
                get(|| async {
                    if let Some(ctx) = context::current() {
                        ctx.push_query(SqlEntry {
                            sql: "SELECT 1".to_string(),
                            duration_ms: 0.5,
                            error: None,
                        });
                    }
                    StatusCode::OK
                }),
            )
            .layer(axum::middleware::from_fn(|req, next| {
                profiler_middleware(true, req, next)
            }))
    }

    /// Build a router with the middleware gated off (`debug == false`).
    fn router_debug_off() -> Router {
        Router::new()
            .route("/profile-me", get(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn(|req, next| {
                profiler_middleware(false, req, next)
            }))
    }

    /// Build a request to `uri`, optionally carrying a request-id header.
    fn request(uri: &str, request_id: Option<&str>) -> HttpRequest<Body> {
        let mut builder = HttpRequest::builder().uri(uri);
        if let Some(request_id) = request_id {
            builder = builder.header(REQUEST_ID_HEADER, request_id);
        }
        builder.body(Body::empty()).expect("request")
    }

    /// Positive: a profiled request lands in the ring with its SQL entry.
    #[tokio::test]
    async fn records_request_with_sql_entry() {
        let _guard = PROFILER_LOCK.lock().await;
        ring::clear_ring();

        let response = router_recording_one_query()
            .oneshot(request("/profile-me", None))
            .await
            .expect("router responds");

        assert_eq!(response.status(), StatusCode::OK);
        let profiles = ring::snapshot();
        let last = profiles.last().expect("one profile recorded");
        assert_eq!(last.method, "GET");
        assert_eq!(last.path, "/profile-me");
        assert_eq!(last.status, 200);
        assert_eq!(last.queries.len(), 1);
        assert_eq!(last.queries[0].sql, "SELECT 1");
        assert!(last.duration_ms >= 0.0);
        ring::clear_ring();
    }

    /// Negative: with `debug == false` no profile is recorded.
    #[tokio::test]
    async fn debug_off_records_nothing() {
        let _guard = PROFILER_LOCK.lock().await;
        ring::clear_ring();

        let response = router_debug_off()
            .oneshot(request("/profile-me", None))
            .await
            .expect("router responds");

        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            ring::snapshot().is_empty(),
            "debug off must not record a profile"
        );
    }

    /// Positive: an `x-request-id` header is captured on the profile.
    #[tokio::test]
    async fn captures_request_id_header() {
        let _guard = PROFILER_LOCK.lock().await;
        ring::clear_ring();

        router_recording_one_query()
            .oneshot(request("/profile-me", Some("req-42")))
            .await
            .expect("router responds");

        let profiles = ring::snapshot();
        let last = profiles.last().expect("one profile recorded");
        assert_eq!(last.request_id.as_deref(), Some("req-42"));
        ring::clear_ring();
    }
}
