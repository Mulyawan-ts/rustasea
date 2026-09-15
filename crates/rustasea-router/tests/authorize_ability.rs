//! Runtime enforcement of the ability-only `#[authorize("users.edit")]` form.
//!
//! Proves the `__RUSTASEA_AUTHORIZE_ABILITY_<Fn>` const emitted by the
//! single-argument `#[authorize]` form is attached to a handler's route and
//! enforced at build time through the ability gate registered with
//! `Router::authorize_abilities`: the check runs after the route's middleware
//! (which populates the principal) and before the handler, returning the
//! authorizer's `403` response without executing the body. Also proves an
//! ability-only declaration without a registered ability gate fails the build
//! closed with a typed error.
//!
//! The `rustasea-router` crate has no dependency on `rustasea-auth`, so the
//! Gate bridge is simulated here with a hand-rolled [`AuthorizeResource`]; the
//! real `Gate`/RBAC bridge is exercised in `rustasea-auth`.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::{IntoResponse, Response};
use rustasea_router::{AuthorizeResource, RouteError, Router};
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

/// Fake ability gate: allow only the principal whose id is `allowed`.
struct AbilityAuthorizer {
    allowed: String,
}

impl AuthorizeResource for AbilityAuthorizer {
    fn authorize(&self, request: &Request, ability: &str) -> Result<(), Response> {
        assert_eq!(ability, "users.edit");
        let principal = request.extensions().get::<Principal>().map(|p| p.0.clone());
        match principal {
            Some(id) if id == self.allowed => Ok(()),
            _ => Err(StatusCode::FORBIDDEN.into_response()),
        }
    }
}

/// The authenticated principal an auth middleware would insert.
#[derive(Clone)]
struct Principal(String);

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

/// Build a router whose `edit_user` route is gated by ability-only `#[authorize]`.
fn gated_router() -> Router {
    let mut router = Router::new();
    router.authorize_abilities(Arc::new(AbilityAuthorizer {
        allowed: "user-1".to_string(),
    }));
    router
        .authorize_ability_meta(gated::__RUSTASEA_AUTHORIZE_ABILITY_edit_user)
        .get_action("/users/:id/edit", gated::edit_user);
    router
}

/// Positive: a permitted principal executes the controller handler.
#[tokio::test]
async fn authorized_user_executes_handler() {
    let app = gated_router().into_axum_router();
    let (status, body) = call(app, get("/users/1/edit", Some("user-1"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "edit-form");
}

/// Negative: a denied principal receives HTTP 403 before the handler executes.
#[tokio::test]
async fn denied_user_is_forbidden_before_handler() {
    let app = gated_router().into_axum_router();
    let (status, body) = call(app, get("/users/1/edit", Some("user-2"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_ne!(body, "edit-form");
}

/// Negative: an unauthenticated request is denied (missing principal).
#[tokio::test]
async fn unauthenticated_is_forbidden() {
    let app = gated_router().into_axum_router();
    let (status, _) = call(app, get("/users/1/edit", None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Negative: ability-only metadata without an ability gate fails the build.
#[test]
fn missing_ability_gate_fails_closed_with_typed_error() {
    let mut router = Router::new();
    router
        .authorize_ability_meta("users.edit")
        .get("/users/:id/edit");

    let error = router
        .try_into_axum_router()
        .expect_err("missing ability gate must fail the build");
    assert_eq!(
        error,
        RouteError::UnknownAuthorization {
            name: "users.edit".to_string()
        }
    );
}

/// The macro emits both the ability const and the legacy tuple const.
#[test]
fn ability_meta_records_ability_name() {
    assert_eq!(gated::__RUSTASEA_AUTHORIZE_ABILITY_edit_user, "users.edit");
    assert_eq!(gated::__RUSTASEA_AUTHORIZE_edit_user, ("users.edit", ""));
}

/// The legacy resource form still resolves through `authorize_meta`.
#[tokio::test]
async fn resource_form_still_works() {
    struct PostAuthorizer;
    impl AuthorizeResource for PostAuthorizer {
        fn authorize(&self, request: &Request, _ability: &str) -> Result<(), Response> {
            if request.extensions().get::<Principal>().is_some() {
                Ok(())
            } else {
                Err(StatusCode::FORBIDDEN.into_response())
            }
        }
    }

    let mut router = Router::new();
    router.register_authorizer("post", Arc::new(PostAuthorizer));
    router.authorize_meta(("update", "post")).get("/posts/:id");
    let app = router.into_axum_router();
    let (status, _) = call(app, get("/posts/1", Some("user-1"))).await;
    assert_eq!(status, StatusCode::OK);
}
