//! End-to-end record-level `#[authorize]` enforcement through the Gate bridge
//! (LARAVEL-010).
//!
//! Wires the real `rustasea_auth` pieces — a `Gate` with a `PolicyRegistry`
//! installed, and the [`GateResource`] router bridge — through the
//! `rustasea_router` authorization layer. Proves the positive path (the resource
//! owner's request reaches the handler) and the negative path (a non-owner is
//! rejected with the Gate's `403` JSON envelope before the body runs).

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use rustasea_auth::{authorizer_for, AuthUser, Gate, Policy, PolicyRegistry};
use rustasea_router::Router;
use tower::ServiceExt;

/// Handler whose `#[authorize("update", &post)]` spec is consumed at registration.
mod gated {
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use rustasea_macros::authorize;

    /// PUT /posts/:id — reachable only when `PostPolicy::update` allows.
    #[authorize("update", &post)]
    pub async fn update_post() -> Response {
        (StatusCode::OK, "updated").into_response()
    }
}

/// A blog post owned by one author.
#[derive(Clone)]
struct Post {
    author_id: String,
}

/// Only the author may update their own post.
struct PostPolicy;

impl Policy<Post> for PostPolicy {
    fn update(&self, user: Option<&AuthUser>, resource: &Post) -> bool {
        user.map(|u| u.id == resource.author_id).unwrap_or(false)
    }
}

/// Build a Gate with the `Post` policy installed for the standard abilities.
fn gate() -> Arc<Gate> {
    let registry = PolicyRegistry::new();
    registry.register::<Post, _>(PostPolicy);
    let mut gate = Gate::new();
    registry.install(&mut gate);
    Arc::new(gate)
}

/// Binding middleware: resolve the post and insert it as a typed extension.
async fn bind_post(mut request: Request, next: Next) -> Response {
    request.extensions_mut().insert(Post {
        author_id: "user-1".to_string(),
    });
    next.run(request).await
}

/// Build a router gating `update_post` through the real Gate bridge.
fn gated_router() -> Router {
    let mut router = Router::new();
    router.register_authorizer("post", authorizer_for::<Post>(gate()));
    router.register_middleware("bind.post", |method_router| {
        method_router.layer(axum::middleware::from_fn(bind_post))
    });
    router
        .middleware_meta(&["bind.post"])
        .authorize_meta(gated::__RUSTASEA_AUTHORIZE_update_post)
        .get_action("/posts/:id", gated::update_post);
    router
}

/// Build a request carrying an optional principal (an auth middleware's job).
fn request(uri: &str, user: Option<AuthUser>) -> HttpRequest<Body> {
    let mut request = HttpRequest::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("request");
    if let Some(user) = user {
        request.extensions_mut().insert(user);
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

/// Positive: the authorized resource owner executes the controller handler.
#[tokio::test]
async fn authorized_owner_executes_handler() {
    let app = gated_router().into_axum_router();
    let owner = AuthUser::new("user-1", Some("user-1@example.com".to_string()), "session");
    let (status, body) = call(app, request("/posts/1", Some(owner))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "updated");
}

/// Negative: a non-owner receives HTTP 403 Forbidden before the handler runs.
#[tokio::test]
async fn non_owner_receives_403() {
    let app = gated_router().into_axum_router();
    let other = AuthUser::new("user-2", Some("user-2@example.com".to_string()), "session");
    let (status, body) = call(app, request("/posts/1", Some(other))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // The Gate's standard JSON envelope is returned, and the handler never ran.
    assert!(body.contains("\"status\":\"403\""), "body was {body:?}");
    assert!(
        body.contains("AuthorizationError::Denied"),
        "body was {body:?}"
    );
    assert_ne!(body, "updated");
}

/// Negative: an unauthenticated request fails closed with 403.
#[tokio::test]
async fn unauthenticated_receives_403() {
    let app = gated_router().into_axum_router();
    let (status, _) = call(app, request("/posts/1", None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
