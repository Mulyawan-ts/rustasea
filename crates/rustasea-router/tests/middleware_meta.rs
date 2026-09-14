//! Runtime consumption of `#[middleware]` attribute metadata (LARAVEL-003).
//!
//! Proves the `__RUSTASEA_MIDDLEWARE_<Fn>` const emitted by the `#[middleware]`
//! attribute is attached to a handler's route and enforced at build time —
//! including parameterized specs (`throttle:60,1`) resolved through a factory,
//! and typed fail-closed errors for unknown or rejected specs.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::MethodRouter;
use rustasea_router::{MiddlewareApply, RouteError, Router};
use tower::ServiceExt;

/// Handler whose `#[middleware("auth")]` spec is consumed at registration.
mod gated {
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use rustasea_macros::middleware;

    /// GET /gated — reachable only when the `auth` middleware passes.
    #[middleware("auth")]
    pub async fn show() -> Response {
        (StatusCode::OK, "gated").into_response()
    }
}

/// Reject requests without an `Authorization` header, mirroring an auth gate.
async fn require_auth(request: Request<Body>, next: Next) -> Response {
    if request.headers().contains_key("authorization") {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

/// Build a plain GET request for `uri`.
fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

/// Drive one request through the compiled router and read status + body.
async fn call(app: axum::Router, request: Request<Body>) -> (StatusCode, String) {
    let response = app.oneshot(request).await.expect("router dispatch");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Positive/negative: `#[middleware("auth")]` on a handler gates its route.
#[tokio::test]
async fn middleware_meta_enforces_declared_middleware() {
    let mut router = Router::new();
    router.register_middleware("auth", |method_router| {
        method_router.layer(axum::middleware::from_fn(require_auth))
    });
    router
        .middleware_meta(gated::__RUSTASEA_MIDDLEWARE_show)
        .get_action("/gated", gated::show);
    let app = router.into_axum_router();

    let (status, _) = call(app.clone(), get("/gated")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let authorized = Request::builder()
        .uri("/gated")
        .header("authorization", "Bearer ok")
        .body(Body::empty())
        .unwrap();
    let (status, body) = call(app, authorized).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "gated");
}

/// A parameterized spec resolves through the registered factory.
#[tokio::test]
async fn parameterized_spec_uses_factory_parameters() {
    let seen = Arc::new(Mutex::new(None::<String>));
    let captured = Arc::clone(&seen);

    let mut router = Router::new();
    router.register_middleware_factory("throttle", move |params| {
        *captured.lock().unwrap() = Some(params.to_string());
        Some(Arc::new(|method_router: MethodRouter<()>| method_router))
    });
    router.middleware_meta(&["throttle:60,1"]).get("/limited");
    let app = router.into_axum_router();

    let (status, _) = call(app, get("/limited")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(seen.lock().unwrap().as_deref(), Some("60,1"));
}

/// Negative: an unregistered middleware spec fails the build with a typed error.
#[test]
fn unknown_middleware_fails_closed_with_typed_error() {
    let mut router = Router::new();
    router.middleware_meta(&["ghost"]).get("/protected");

    let error = router
        .try_into_axum_router()
        .expect_err("unknown middleware must fail the build");
    assert_eq!(
        error,
        RouteError::UnknownMiddleware {
            name: "ghost".to_string()
        }
    );
}

/// Negative: a factory that rejects its parameters fails the build closed.
#[test]
fn rejected_parameters_fail_closed_with_typed_error() {
    let mut router = Router::new();
    router.register_middleware_factory("throttle", |_params: &str| -> Option<MiddlewareApply> {
        None
    });
    router.middleware_meta(&["throttle:bad"]).get("/limited");

    let error = router
        .try_into_axum_router()
        .expect_err("rejected parameters must fail the build");
    assert_eq!(
        error,
        RouteError::UnknownMiddleware {
            name: "throttle:bad".to_string()
        }
    );
}
