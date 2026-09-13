//! Reusable HTTP test client (feature `http`).
//!
//! Generalises the inline tower-oneshot JSON request pattern used across the
//! feature suites (`crates/rustasea/tests/feature/route_to_db.rs`): build a
//! request, drive an [`axum::Router`] with a single
//! [`tower::ServiceExt::oneshot`] dispatch, collect the response body, and
//! decode it as JSON. Feature tests get one call instead of repeating the
//! `Body`/`to_bytes`/`serde_json` boilerplate.
//!
//! Enable with the `http` cargo feature.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

/// Send one request through `router` and decode the response as JSON.
///
/// `method` is an HTTP verb (`"GET"`, `"POST"`, …) and `uri` the request
/// target (`"/users/1"`). When `body` is `Some`, it is serialized to JSON and
/// sent with `content-type: application/json`; when `None`, an empty body is
/// sent. The response body is read to completion and parsed with
/// [`serde_json`]; a non-JSON body (or an empty one) decodes to
/// [`serde_json::Value::Null`], so a `204`/`404` still returns its status.
///
/// # Panics
///
/// Panics when `method`/`uri` are not valid HTTP request components or when
/// the router fails to produce a response — both are test-authoring errors, so
/// failing fast matches the rest of the harness.
pub async fn request_json(
    router: Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let builder = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(value) => builder
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&value).expect("request body serializes to JSON"),
            ))
            .expect("request builds"),
        None => builder.body(Body::empty()).expect("request builds"),
    };

    let response = router.oneshot(request).await.expect("router dispatch");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("response body collects")
        .to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::routing::{get, post};
    use axum::Json;
    use serde_json::json;

    /// A tiny router: `GET /ping` → `{"pong": true}`, `POST /echo` → the body.
    fn test_router() -> Router {
        Router::new()
            .route("/ping", get(|| async { Json(json!({ "pong": true })) }))
            .route(
                "/echo",
                post(|Json(body): Json<serde_json::Value>| async move { Json(body) }),
            )
    }

    /// `request_json` drives a GET and decodes the JSON body.
    #[tokio::test]
    async fn get_decodes_json_body() {
        let (status, body) = request_json(test_router(), "GET", "/ping", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["pong"], json!(true));
    }

    /// `request_json` sends a JSON body and round-trips it through the handler.
    #[tokio::test]
    async fn post_round_trips_json_body() {
        let payload = json!({ "name": "Ada", "age": 36 });
        let (status, body) =
            request_json(test_router(), "POST", "/echo", Some(payload.clone())).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, payload);
    }

    /// An unrouted path yields the router's own `404` (null body → `Null`).
    #[tokio::test]
    async fn unmatched_route_returns_404() {
        let (status, body) = request_json(test_router(), "GET", "/missing", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body, serde_json::Value::Null);
    }
}
