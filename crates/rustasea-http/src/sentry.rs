//! Sentry request-context middleware (ADOPT-004) — opt-in via the `sentry`
//! feature.
//!
//! [`sentry_context_middleware`] enriches every request with Sentry context
//! (method, path, status, request id) and captures a Sentry event for any
//! response with a 5xx status. When no Sentry client is bound — the default, and
//! the only state when no DSN is configured — every `sentry::*` call here is a
//! documented no-op, so the layer costs nothing beyond the tag bookkeeping and
//! the request passes through unchanged.
//!
//! # Request-local hub (no scope leakage)
//!
//! Sentry's `Hub::current()` is **thread-local**, and an async handler may
//! resume on a different worker thread after an `await`. Mutating the current
//! hub's scope (`configure_scope`) therefore both leaks request tags onto every
//! later request served by that worker thread and can be corrupted by
//! await-point thread migration.
//!
//! This middleware never touches the thread-local hub. At entry it snapshots
//! the process client into a **request-local** hub
//! ([`sentry::Hub::new_from_top`]) and confines every scope mutation to that
//! hub. `next.run(request).await` runs outside any scope mutation, and the 5xx
//! capture configures the request-local hub's scope and captures through it.
//! Because the hub is dropped when the middleware returns, no context can
//! survive past the response. `Hub::with_scope` is synchronous-only (it cannot
//! hold a scope across `.await`) and `Hub::run` binds the hub to the *thread*,
//! so neither is suitable here.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::Method;
use axum::middleware::Next;
use axum::response::Response;

use sentry::protocol::{Event, Level};

/// Request-id header read by the middleware.
const REQUEST_ID_HEADER: &str = "x-request-id";

/// Tag key for the HTTP method.
const TAG_METHOD: &str = "http.method";
/// Tag key for the request path.
const TAG_PATH: &str = "http.path";
/// Tag key for the response status code.
const TAG_STATUS: &str = "http.status_code";
/// Tag key for the request id.
const TAG_REQUEST_ID: &str = "http.request_id";

/// Middleware that tags request context and captures 5xx responses.
///
/// The middleware:
///
/// 1. Reads the method, path, and optional `x-request-id` header.
/// 2. Snapshots the process client into a request-local [`sentry::Hub`].
/// 3. Runs the inner service **without** mutating any scope.
/// 4. Captures a Sentry error event when the response is a server error (5xx),
///    configuring the request-local hub's scope with the request context first.
///
/// No client bound means step 4 is a no-op; the response is returned unchanged.
pub async fn sentry_context_middleware(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let request_id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    // Request-local hub: it inherits the process client from the thread-local
    // hub but keeps its own scope. Dropping it disposes all request context.
    let hub = Arc::new(sentry::Hub::new_from_top(sentry::Hub::current()));

    let response = next.run(request).await;
    let status = response.status();

    if status.is_server_error() {
        capture_server_error(&hub, &method, &path, request_id.as_deref(), status.as_u16());
    }

    response
}

/// Capture a Sentry error event for a 5xx response on the request-local `hub`.
///
/// The request context is set on the hub's own scope, then the event is captured
/// through the hub so the scope is applied. Both the scope mutation and the
/// capture are request-local, so neither can leak onto the worker thread.
fn capture_server_error(
    hub: &sentry::Hub,
    method: &Method,
    path: &str,
    request_id: Option<&str>,
    status: u16,
) {
    hub.configure_scope(|scope| {
        scope.set_transaction(Some(path));
        scope.set_tag(TAG_METHOD, method.as_str());
        scope.set_tag(TAG_PATH, path);
        scope.set_tag(TAG_STATUS, status.to_string());
        if let Some(request_id) = request_id {
            scope.set_tag(TAG_REQUEST_ID, request_id);
        }
    });

    let event = Event {
        level: Level::Error,
        message: Some(format!("{method} {path} -> {status}")),
        transaction: Some(path.to_string()),
        ..Default::default()
    };
    hub.capture_event(event);
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use sentry::{ClientOptions, Envelope, Hub, Transport};
    use tower::ServiceExt;

    use super::*;

    /// Serializes tests that bind a client to the thread-local Sentry hub.
    ///
    /// An async-aware mutex is used because the guard is held across the
    /// router's `.await`.
    static HUB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// A Sentry transport that records every envelope instead of sending it.
    #[derive(Default)]
    struct CapturingTransport {
        envelopes: Mutex<Vec<Envelope>>,
    }

    impl CapturingTransport {
        /// Number of envelopes received so far.
        fn len(&self) -> usize {
            self.envelopes.lock().expect("transport lock").len()
        }

        /// Every captured event, in order.
        fn events(&self) -> Vec<Event<'static>> {
            self.envelopes
                .lock()
                .expect("transport lock")
                .iter()
                .filter_map(Envelope::event)
                .cloned()
                .collect()
        }

        /// The first captured event, if any.
        fn first_event(&self) -> Option<Event<'static>> {
            self.events().into_iter().next()
        }
    }

    impl Transport for CapturingTransport {
        fn send_envelope(&self, envelope: Envelope) {
            self.envelopes
                .lock()
                .expect("transport lock")
                .push(envelope);
        }
    }

    /// Build a router with the middleware in front of a handler returning
    /// `status`.
    fn router_returning(status: StatusCode) -> Router {
        Router::new()
            .route("/boom", get(move || async move { status }))
            .layer(axum::middleware::from_fn(sentry_context_middleware))
    }

    /// Build a request to `/boom`, optionally carrying a request-id header.
    fn request_with_id(request_id: Option<&str>) -> HttpRequest<Body> {
        let mut builder = HttpRequest::builder().uri("/boom");
        if let Some(request_id) = request_id {
            builder = builder.header(REQUEST_ID_HEADER, request_id);
        }
        builder.body(Body::empty()).expect("request")
    }

    /// Bind a capturing client to the current thread's hub.
    fn bind_capturing_client() -> Arc<CapturingTransport> {
        let transport = Arc::new(CapturingTransport::default());
        // `TransportFactory` is implemented for `Arc<T>`; the builder wraps the
        // value in the `Arc<dyn TransportFactory>` the options expect.
        let mut options = ClientOptions::new().transport(transport.clone());
        options.dsn = Some(
            "https://public@example.com/1"
                .parse()
                .expect("valid test DSN"),
        );
        let client = Arc::new(sentry::Client::from_config(options));
        Hub::current().bind_client(Some(client));
        transport
    }

    /// Remove any client bound to the current thread's hub.
    fn unbind_client() {
        Hub::current().bind_client(None);
    }

    #[tokio::test]
    async fn captures_5xx_with_request_context() {
        let _guard = HUB_LOCK.lock().await;
        let transport = bind_capturing_client();

        let response = router_returning(StatusCode::INTERNAL_SERVER_ERROR)
            .oneshot(request_with_id(Some("req-42")))
            .await
            .expect("router responds");

        unbind_client();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(transport.len(), 1, "one 5xx event must be captured");
        let event = transport.first_event().expect("captured event");
        assert_eq!(event.level, Level::Error);
        assert_eq!(event.transaction.as_deref(), Some("/boom"));
        assert_eq!(event.tags.get(TAG_METHOD).map(String::as_str), Some("GET"));
        assert_eq!(event.tags.get(TAG_PATH).map(String::as_str), Some("/boom"));
        assert_eq!(event.tags.get(TAG_STATUS).map(String::as_str), Some("500"));
        assert_eq!(
            event.tags.get(TAG_REQUEST_ID).map(String::as_str),
            Some("req-42")
        );
    }

    #[tokio::test]
    async fn does_not_leak_context_between_requests() {
        let _guard = HUB_LOCK.lock().await;
        let transport = bind_capturing_client();

        // Request A carries a request id and produces a 5xx event.
        router_returning(StatusCode::INTERNAL_SERVER_ERROR)
            .oneshot(request_with_id(Some("abc")))
            .await
            .expect("router responds");
        // Request B carries none; it must not inherit A's context.
        router_returning(StatusCode::INTERNAL_SERVER_ERROR)
            .oneshot(request_with_id(None))
            .await
            .expect("router responds");

        unbind_client();

        let events = transport.events();
        assert_eq!(events.len(), 2, "two 5xx events must be captured");
        assert_eq!(
            events[0].tags.get(TAG_REQUEST_ID).map(String::as_str),
            Some("abc"),
            "the first request keeps its own request id"
        );
        assert!(
            !events[1].tags.contains_key(TAG_REQUEST_ID),
            "the second request must not inherit the first's request id: {:?}",
            events[1].tags
        );
    }

    #[tokio::test]
    async fn passes_through_2xx_without_capturing() {
        let _guard = HUB_LOCK.lock().await;
        let transport = bind_capturing_client();

        let response = router_returning(StatusCode::OK)
            .oneshot(request_with_id(None))
            .await
            .expect("router responds");

        unbind_client();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(transport.len(), 0, "2xx must not be captured");
    }

    #[tokio::test]
    async fn works_without_a_bound_client() {
        let _guard = HUB_LOCK.lock().await;
        // No client bound: every sentry call is a no-op and must not panic.
        let response = router_returning(StatusCode::INTERNAL_SERVER_ERROR)
            .oneshot(request_with_id(None))
            .await
            .expect("router responds");

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
