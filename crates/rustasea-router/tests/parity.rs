//! Route parity acceptance tests (RTE-001).
//!
//! Covers named routes with group-prefix composition, reverse URL resolution,
//! redirect helpers, and per-route / per-group middleware enforcement at build
//! time — including the typed `RouteError::UnknownMiddleware` failure and the
//! fail-closed `into_axum_router` fallback.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use rustasea_router::{RouteError, Router};
use tower::ServiceExt;

/// Drive one request through the compiled router and read status + headers.
async fn call(app: axum::Router, request: Request<Body>) -> (StatusCode, axum::http::HeaderMap) {
    let response = app.oneshot(request).await.expect("router dispatch");
    let status = response.status();
    (status, response.headers().clone())
}

/// Build a plain GET request for `uri`.
fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

/// Plain routes can be named and the group prefix composes.
#[test]
fn named_route_composes_with_group_prefix() {
    let mut router = Router::new();
    router.name("settings.");
    router.get("/profile").named("profile.edit");

    let names: Vec<String> = router
        .get_routes()
        .iter()
        .filter_map(|r| r.name.clone())
        .collect();
    assert_eq!(names, vec!["settings.profile.edit".to_string()]);
}

/// `any` names every method in its batch, and the reverse index resolves it.
#[test]
fn any_batch_is_named_and_resolvable() {
    let mut router = Router::new();
    router.any("/hook").named("hooks.receive");

    let routes = router.get_routes();
    assert_eq!(routes.len(), 6);
    assert!(routes
        .iter()
        .all(|r| r.name.as_deref() == Some("hooks.receive")));
    assert_eq!(router.url("hooks.receive", &[]).as_deref(), Some("/hook"));
}

/// A route named inside a group closure inherits the group prefix.
#[test]
fn group_scoped_name_prefix_composes() {
    let mut router = Router::new();
    router.group(|group| {
        group.name("settings.");
        group.get("/profile").named("profile.edit");
    });

    assert_eq!(
        router.url("settings.profile.edit", &[]).as_deref(),
        Some("/profile")
    );
}

/// Resource routes compose with the group name prefix.
#[test]
fn resource_names_compose_with_group_prefix() {
    let mut router = Router::new();
    router.group(|group| {
        group.name("admin.");
        group.resource("users", "UserController");
    });

    assert_eq!(
        router.url("admin.users.show", &[("id", "7")]).as_deref(),
        Some("/users/7")
    );
}

/// Known name resolves to a path; params are substituted; unknown → None.
#[test]
fn url_resolution_known_params_and_unknown() {
    let mut router = Router::new();
    router.get("/users/{id}").named("users.show");

    assert_eq!(
        router.url("users.show", &[("id", "42")]).as_deref(),
        Some("/users/42")
    );
    assert_eq!(router.url("users.show", &[]), None);
    assert_eq!(router.url("missing", &[]), None);
}

/// `redirect` emits 302 with a correct Location header.
#[tokio::test]
async fn redirect_defaults_to_302() {
    let mut router = Router::new();
    router.redirect("/old", "/new");
    let app = router.into_axum_router();

    let (status, headers) = call(app, get("/old")).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get("location"),
        Some(&HeaderValue::from_static("/new"))
    );
}

/// `redirect_permanent` emits 301 with a correct Location header.
#[tokio::test]
async fn redirect_permanent_emits_301() {
    let mut router = Router::new();
    router.redirect_permanent("/legacy", "/modern");
    let app = router.into_axum_router();

    let (status, headers) = call(app, get("/legacy")).await;
    assert_eq!(status, StatusCode::MOVED_PERMANENTLY);
    assert_eq!(
        headers.get("location"),
        Some(&HeaderValue::from_static("/modern"))
    );
}

/// Register a header-injecting middleware under `name` for the given router.
fn register_header_middleware(router: &mut Router, name: &'static str, header: &'static str) {
    router.register_middleware(name, move |method_router| {
        method_router.layer(middleware::from_fn(
            move |request: Request<Body>, next: Next| async move {
                let mut response = next.run(request).await;
                response
                    .headers_mut()
                    .insert(header, HeaderValue::from_static("on"));
                response
            },
        ))
    });
}

/// A registered middleware runs for a plain route that declares it.
#[tokio::test]
async fn registered_middleware_runs_for_plain_route() {
    let mut router = Router::new();
    register_header_middleware(&mut router, "stamp", "x-stamp");
    router.middleware("stamp").get("/protected");
    let app = router.into_axum_router();

    let (status, headers) = call(app, get("/protected")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("x-stamp"),
        Some(&HeaderValue::from_static("on"))
    );
}

/// A group-level middleware is inherited by routes registered in the group.
#[tokio::test]
async fn group_middleware_is_inherited() {
    let mut router = Router::new();
    register_header_middleware(&mut router, "stamp", "x-stamp");
    router.group(|group| {
        group.middleware("stamp");
        group.get("/inside");
    });
    let app = router.into_axum_router();

    let (status, headers) = call(app, get("/inside")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("x-stamp"),
        Some(&HeaderValue::from_static("on"))
    );
}

/// First-declared middleware is outermost (runs first on the request).
#[tokio::test]
async fn middleware_order_is_first_declared_outermost() {
    let log = Arc::new(Mutex::new(Vec::<String>::new()));

    let mut router = Router::new();
    for name in ["first", "second"] {
        let log = Arc::clone(&log);
        let label = name.to_string();
        router.register_middleware(name, move |method_router| {
            let log = Arc::clone(&log);
            let label = label.clone();
            method_router.layer(middleware::from_fn(
                move |request: Request<Body>, next: Next| {
                    let log = Arc::clone(&log);
                    let label = label.clone();
                    async move {
                        log.lock().unwrap().push(label);
                        next.run(request).await
                    }
                },
            ))
        });
    }
    router
        .middleware("first")
        .middleware("second")
        .get("/ordered");
    let app = router.into_axum_router();

    let (status, _) = call(app, get("/ordered")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        *log.lock().unwrap(),
        vec!["first".to_string(), "second".to_string()]
    );
}

/// An unknown middleware identifier is a typed build error.
#[test]
fn unknown_middleware_is_typed_error() {
    let mut router = Router::new();
    router.middleware("ghost").get("/protected");

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

/// `into_axum_router` fails closed (500) on an unknown middleware.
#[tokio::test]
async fn unknown_middleware_fails_closed_with_500() {
    let mut router = Router::new();
    router.middleware("ghost").get("/protected");
    let app = router.into_axum_router();

    let (status, _) = call(app, get("/protected")).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

/// Middleware does not disturb the stub handler body for unbound routes.
#[tokio::test]
async fn middleware_preserves_route_body() {
    let mut router = Router::new();
    register_header_middleware(&mut router, "stamp", "x-stamp");
    router.middleware("stamp").get("/body");
    let app = router.into_axum_router();

    let response = app.oneshot(get("/body")).await.expect("dispatch");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    assert_eq!(&bytes[..], b"ok");
}

/// A custom handler still runs under a middleware layer.
#[tokio::test]
async fn middleware_wraps_real_action() {
    async fn hello() -> Response {
        "hello".into_response()
    }

    let mut router = Router::new();
    register_header_middleware(&mut router, "stamp", "x-stamp");
    router.middleware("stamp").get_action("/hello", hello);
    let app = router.into_axum_router();

    let response = app.oneshot(get("/hello")).await.expect("dispatch");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("x-stamp"),
        Some(&HeaderValue::from_static("on"))
    );
}
