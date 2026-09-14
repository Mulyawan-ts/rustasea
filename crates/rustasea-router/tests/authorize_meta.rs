//! Runtime enforcement of `#[authorize]` attribute metadata (LARAVEL-010).
//!
//! Proves the `__RUSTASEA_AUTHORIZE_<Fn>` const emitted by the `#[authorize]`
//! attribute is attached to a handler's route and enforced at build time: the
//! authorization layer runs after the route's middleware (which populates the
//! principal + resolved resource) and before the handler, returning the
//! authorizer's `403` response without executing the body. Also proves an
//! unregistered resource id fails the build closed with a typed error.
//!
//! The `rustasea-router` crate has no dependency on `rustasea-auth`, so the
//! Gate bridge is simulated here with a hand-rolled [`AuthorizeResource`]; the
//! real `Gate`/`Policy` bridge is exercised in `rustasea-auth`.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rustasea_router::{AuthorizeResource, RouteError, Router};
use tower::ServiceExt;

/// Handler whose `#[authorize("update", &post)]` spec is consumed at registration.
mod gated {
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use rustasea_macros::authorize;

    /// PUT /posts/:id — reachable only when the `update` ability passes.
    #[authorize("update", &post)]
    pub async fn update_post() -> Response {
        (StatusCode::OK, "updated").into_response()
    }
}

/// The resource a binding middleware would resolve and insert.
#[derive(Clone)]
struct Post {
    author_id: String,
}

/// Fake authorizer: allow when the request principal id owns the resolved post.
struct PostAuthorizer;

impl AuthorizeResource for PostAuthorizer {
    fn authorize(&self, request: &Request, ability: &str) -> Result<(), Response> {
        assert_eq!(ability, "update");
        let owner = request
            .extensions()
            .get::<Post>()
            .map(|post| post.author_id.clone());
        let principal = request.extensions().get::<Principal>().map(|p| p.0.clone());
        match (owner, principal) {
            (Some(owner), Some(principal)) if owner == principal => Ok(()),
            _ => Err(StatusCode::FORBIDDEN.into_response()),
        }
    }
}

/// The authenticated principal an auth middleware would insert.
#[derive(Clone)]
struct Principal(String);

/// Binding middleware: resolve the post and insert it as a typed extension.
async fn bind_post(mut request: Request, next: Next) -> Response {
    // A real binding middleware loads the model from the path id; the owner is
    // fixed here so the test is deterministic.
    request.extensions_mut().insert(Post {
        author_id: "user-1".to_string(),
    });
    next.run(request).await
}

/// Build a plain GET request for `uri`, optionally carrying a principal id.
fn get(uri: &str, principal: Option<&str>) -> HttpRequest<Body> {
    let mut request = HttpRequest::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("request");
    if let Some(id) = principal {
        request.extensions_mut().insert(Principal(id.to_string()));
    }
    request
}

/// Drive one request through the compiled router and read status + body.
async fn call(app: axum::Router, request: HttpRequest<Body>) -> (StatusCode, String) {
    let response = app.oneshot(request).await.expect("router dispatch");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Build a router whose `update_post` route is gated by `#[authorize]`.
fn gated_router() -> Router {
    let mut router = Router::new();
    router.register_authorizer("post", Arc::new(PostAuthorizer));
    // The `bind_post` middleware populates the typed resource extension the
    // authorizer reads; `#[authorize]` runs innermost so it sees it.
    router.register_middleware("bind.post", |method_router| {
        method_router.layer(axum::middleware::from_fn(bind_post))
    });
    router
        .middleware_meta(&["bind.post"])
        .authorize_meta(gated::__RUSTASEA_AUTHORIZE_update_post)
        .get_action("/posts/:id", gated::update_post);
    router
}

/// Positive: the authorized owner executes the controller handler.
#[tokio::test]
async fn authorized_owner_executes_handler() {
    let app = gated_router().into_axum_router();
    let (status, body) = call(app, get("/posts/1", Some("user-1"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "updated");
}

/// Negative: a non-owner receives HTTP 403 before the handler executes.
#[tokio::test]
async fn non_owner_is_forbidden_before_handler() {
    let app = gated_router().into_axum_router();
    let (status, body) = call(app, get("/posts/1", Some("user-2"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // The handler body never ran, so its `"updated"` payload is absent.
    assert_ne!(body, "updated");
}

/// Negative: an unauthenticated request is denied (missing principal).
#[tokio::test]
async fn unauthenticated_is_forbidden() {
    let app = gated_router().into_axum_router();
    let (status, _) = call(app, get("/posts/1", None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Negative: an unregistered `#[authorize]` resource fails the build closed.
#[test]
fn unknown_resource_fails_closed_with_typed_error() {
    let mut router = Router::new();
    router.authorize_meta(("update", "ghost")).get("/posts/:id");

    let error = router
        .try_into_axum_router()
        .expect_err("unregistered authorization resource must fail the build");
    assert_eq!(
        error,
        RouteError::UnknownAuthorization {
            name: "ghost".to_string()
        }
    );
}

/// The `#[authorize]` metadata records the ability and the unwrapped resource id.
#[test]
fn authorize_meta_records_ability_and_resource() {
    assert_eq!(gated::__RUSTASEA_AUTHORIZE_update_post, ("update", "post"));
}
