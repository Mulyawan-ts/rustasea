//! Implicit model route binding tests (ADOPT-016).
//!
//! Proves a registered [`ModelBinder`] for a `{param:field}` selector resolves
//! the path value, publishes the model into the request extensions (read by the
//! handler via `axum::Extension`), and that a binder miss returns its response
//! (404) before the handler runs. Binding is opt-in: an unregistered selector
//! falls back to a plain `:param` path parameter rather than failing the build.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Extension, Path, Request};
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::{IntoResponse, Response};
use rustasea_router::{ModelBinder, Router};
use tower::ServiceExt;

/// A bound model published into the request extensions.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Post {
    /// The slug the model was resolved by.
    slug: String,
}

/// Resolves a `Post` from the path slug, publishing it into the extensions.
struct PostBinder {
    /// The slug that resolves; any other value is a miss.
    known: String,
}

impl ModelBinder for PostBinder {
    fn bind(&self, request: &mut Request, value: &str) -> Result<(), Response> {
        if value == self.known {
            request.extensions_mut().insert(Post {
                slug: value.to_string(),
            });
            Ok(())
        } else {
            Err(StatusCode::NOT_FOUND.into_response())
        }
    }
}

/// GET /posts/{post:slug} — reads the bound model from the extensions.
async fn show_post(Extension(post): Extension<Post>) -> Response {
    (StatusCode::OK, format!("post:{}", post.slug)).into_response()
}

/// GET /posts/{post:slug} — reads the raw path param (no bound model).
async fn show_raw(Path(params): Path<HashMap<String, String>>) -> Response {
    let slug = params.get("post").cloned().unwrap_or_default();
    (StatusCode::OK, format!("raw:{slug}")).into_response()
}

/// Build a plain GET request for `uri`.
fn get(uri: &str) -> HttpRequest<Body> {
    HttpRequest::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("request")
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

/// Positive: the binder resolves and the handler reads the bound model.
#[tokio::test]
async fn registered_binder_publishes_model_for_handler() {
    let mut router = Router::new();
    router.register_binding(
        "slug",
        Arc::new(PostBinder {
            known: "hello-world".to_string(),
        }),
    );
    router.get_action("/posts/{post:slug}", show_post);
    let app = router.into_axum_router();

    let (status, body) = call(app, get("/posts/hello-world")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "post:hello-world");
}

/// Positive: binding composes with a plain path param on the same route.
#[tokio::test]
async fn binding_and_plain_param_coexist() {
    let mut router = Router::new();
    router.register_binding(
        "slug",
        Arc::new(PostBinder {
            known: "ada".to_string(),
        }),
    );
    router.get_action("/teams/{team}/posts/{post:slug}", show_post);
    let app = router.into_axum_router();

    let (status, body) = call(app, get("/teams/7/posts/ada")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "post:ada");
}

/// Negative: a binder miss returns its 404 response before the handler runs.
#[tokio::test]
async fn binder_miss_returns_404() {
    let mut router = Router::new();
    router.register_binding(
        "slug",
        Arc::new(PostBinder {
            known: "present".to_string(),
        }),
    );
    router.get_action("/posts/{post:slug}", show_post);
    let app = router.into_axum_router();

    let (status, body) = call(app, get("/posts/absent")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_ne!(body, "post:absent");
}

/// GET /teams/{team}/members/{member:slug} — reads the raw path params and
/// reports whether a bound `Post` leaked into the extensions.
async fn show_raw_member(
    Path(params): Path<HashMap<String, String>>,
    request: Request,
) -> Response {
    let team = params.get("team").cloned().unwrap_or_default();
    let member = params.get("member").cloned().unwrap_or_default();
    let bound = request.extensions().get::<Post>().is_some();
    (StatusCode::OK, format!("raw:{team}/{member}/bound:{bound}")).into_response()
}

/// Opt-in: a selector with no registered binder builds and falls back to a
/// plain path param — the handler reads the raw `:member` value and no model is
/// published into the extensions.
#[tokio::test]
async fn unregistered_selector_falls_back_to_plain_path_param() {
    let mut router = Router::new();
    router.get_action("/teams/{team}/members/{member:slug}", show_raw_member);
    // Build must succeed despite the unregistered `slug` selector.
    let app = router.into_axum_router();

    let (status, body) = call(app, get("/teams/7/members/ada")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "raw:7/ada/bound:false");
}

/// A route with no `{param:field}` selector is untouched by the binding layer.
#[tokio::test]
async fn plain_route_needs_no_binder() {
    let mut router = Router::new();
    router.get_action("/posts/{post}", show_raw);
    let app = router.into_axum_router();

    let (status, body) = call(app, get("/posts/anything")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "raw:anything");
}
