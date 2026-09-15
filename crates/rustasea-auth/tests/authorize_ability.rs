//! End-to-end ability-only `#[authorize("users.edit")]` enforcement through the
//! real Gate + RBAC bridge.
//!
//! Wires a [`Gate`] with an [`RbacRegistry`] installed as its permission
//! resolver and the [`AbilityGateResource`] router bridge, then proves the
//! positive path (a user whose role grants `users.edit` reaches the handler)
//! and the negative path (a user without the permission is rejected with the
//! Gate's `403` JSON envelope before the body runs).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use rustasea_auth::{authorizer_for_abilities, AuthUser, Gate, RbacRegistry, Role};
use rustasea_router::Router;
use tower::ServiceExt;

/// Handler whose ability-only `#[authorize("users.edit")]` spec is consumed.
mod gated {
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use rustasea_macros::authorize;

    /// GET /users/:id/edit — reachable only when `users.edit` passes.
    #[authorize("users.edit")]
    pub async fn edit_user() -> Response {
        (StatusCode::OK, "edit-form").into_response()
    }
}

/// Build an RBAC registry where `user-1` holds a role granting `users.edit`.
fn rbac() -> Arc<RbacRegistry> {
    let mut registry = RbacRegistry::new();
    registry.define_permission("users.edit").expect("define");
    registry
        .define_role(Role::new("admin").with_permissions(["users.edit"]))
        .expect("role");
    registry.assign_role("user-1", "admin").expect("assign");
    Arc::new(registry)
}

/// Build a Gate with the RBAC registry installed as its permission resolver.
fn gate() -> Arc<Gate> {
    Arc::new(Gate::new().with_permissions(rbac()))
}

/// Build a router gating `edit_user` through the real ability-gate bridge.
fn gated_router() -> Router {
    let mut router = Router::new();
    router.authorize_abilities(authorizer_for_abilities(gate()));
    router
        .authorize_ability_meta(gated::__RUSTASEA_AUTHORIZE_ABILITY_edit_user)
        .get_action("/users/:id/edit", gated::edit_user);
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

/// Positive: a user whose role grants the permission executes the handler.
#[tokio::test]
async fn authorized_user_executes_handler() {
    let app = gated_router().into_axum_router();
    let admin = AuthUser::new("user-1", Some("admin@example.com".to_string()), "session");
    let (status, body) = call(app, request("/users/1/edit", Some(admin))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "edit-form");
}

/// Negative: a user without the permission receives HTTP 403 before the body.
#[tokio::test]
async fn unauthorized_user_receives_403() {
    let app = gated_router().into_axum_router();
    let other = AuthUser::new("user-2", Some("other@example.com".to_string()), "session");
    let (status, body) = call(app, request("/users/1/edit", Some(other))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // The Gate's standard JSON envelope is returned, and the handler never ran.
    assert!(body.contains("\"status\":\"403\""), "body was {body:?}");
    assert!(
        body.contains("AuthorizationError::Denied"),
        "body was {body:?}"
    );
    assert_ne!(body, "edit-form");
}

/// Negative: an unauthenticated request fails closed with 403.
#[tokio::test]
async fn unauthenticated_receives_403() {
    let app = gated_router().into_axum_router();
    let (status, _) = call(app, request("/users/1/edit", None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
