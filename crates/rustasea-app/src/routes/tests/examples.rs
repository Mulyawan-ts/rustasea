//! Example domain tests - the `/examples/posts` JSON CRUD surface.
//!
//! Drives the compiled router through `tower::ServiceExt::oneshot` exactly like
//! the sibling route tests. The in-memory [`PostRepository`] is process-wide, so
//! every case resets it first and the mutating cases are serialized through the
//! repository's shared test lock to stay deterministic under libtest's parallel
//! runner.
//!
//! [`PostRepository`]: crate::app::models::PostRepository

use axum::body::Body;
use axum::http::{header, HeaderValue, Request, StatusCode};

use super::{app, call, csrf_same_origin, method_request};
use crate::app::models::post::test_lock;
use crate::app::models::PostRepository;

/// Acquire the example-test serialization mutex.
///
/// Shared with the model and action tests (the store is process-wide), so every
/// case that mutates the repository is mutually exclusive.
async fn lock_examples() -> tokio::sync::MutexGuard<'static, ()> {
    test_lock().lock().await
}

/// A JSON request with an explicit method, body, and content type.
fn json_request(method: &str, uri: &str, body: &str) -> Request<Body> {
    let mut request = method_request(method, uri);
    request.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    *request.body_mut() = Body::from(body.to_string());
    request
}

/// Positive: `GET /examples/posts` answers `200` with a JSON array.
#[tokio::test]
async fn index_returns_a_json_array() {
    let _guard = lock_examples().await;
    PostRepository::reset();

    let (status, _, body) = call(app(), super::get("/examples/posts")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.trim(), "[]", "an empty store must serialize to []");
}

/// Positive: `POST` creates a post, `GET show` reads it back, and `DELETE`
/// removes it.
#[tokio::test]
async fn create_show_and_delete_round_trip() {
    let _guard = lock_examples().await;
    PostRepository::reset();

    let payload = r#"{"title":"Hello","body":"World","published":true}"#;
    let (status, _, body) = call(
        app(),
        csrf_same_origin(json_request("POST", "/examples/posts", payload)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    assert!(body.contains(r#""title":"Hello""#), "body: {body}");

    let id = serde_json::from_str::<serde_json::Value>(&body)
        .expect("create body is JSON")
        .get("id")
        .and_then(|value| value.as_str())
        .expect("the created post must carry an id")
        .to_string();

    let (status, _, body) = call(app(), super::get(&format!("/examples/posts/{id}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(r#""title":"Hello""#), "body: {body}");

    let (status, _, body) = call(
        app(),
        csrf_same_origin(method_request("DELETE", &format!("/examples/posts/{id}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(r#""deleted":true"#), "body: {body}");

    let (status, _, _) = call(app(), super::get(&format!("/examples/posts/{id}"))).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the deleted post must be gone"
    );
}

/// Negative: `GET show` for an unknown id answers `404`.
#[tokio::test]
async fn show_unknown_id_is_404() {
    let _guard = lock_examples().await;
    PostRepository::reset();

    let missing = uuid::Uuid::now_v7();
    let (status, _, body) = call(app(), super::get(&format!("/examples/posts/{missing}"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("No post with id"), "body: {body}");
}

/// Negative: `POST` with a blank title/body answers `422` with the error
/// envelope.
#[tokio::test]
async fn store_invalid_payload_is_422() {
    let _guard = lock_examples().await;
    PostRepository::reset();

    let payload = r#"{"title":"","body":"","published":false}"#;
    let (status, _, body) = call(
        app(),
        csrf_same_origin(json_request("POST", "/examples/posts", payload)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
    assert!(
        body.contains(r#""errors""#) && body.contains(r#""title""#),
        "a field error envelope is required; body: {body}"
    );
    assert!(
        PostRepository::list().is_empty(),
        "an invalid payload must not persist a row"
    );
}

/// Negative: a `POST` with no CSRF token is rejected `403` before the handler.
#[tokio::test]
async fn store_without_csrf_token_is_rejected() {
    let _guard = lock_examples().await;
    PostRepository::reset();

    let payload = r#"{"title":"Hello","body":"World","published":false}"#;
    let mut request = json_request("POST", "/examples/posts", payload);
    request
        .headers_mut()
        .insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
    let (status, _, _) = call(app(), request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        PostRepository::list().is_empty(),
        "a CSRF-rejected write must not persist a row"
    );
}

/// Positive: `PUT` updates an existing post.
#[tokio::test]
async fn update_replaces_fields() {
    let _guard = lock_examples().await;
    PostRepository::reset();

    let created = PostRepository::create("Before".to_string(), "Body".to_string(), false);
    let payload = r#"{"title":"After","body":"Updated","published":true}"#;
    let (status, _, body) = call(
        app(),
        csrf_same_origin(json_request(
            "PUT",
            &format!("/examples/posts/{}", created.id),
            payload,
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(body.contains(r#""title":"After""#), "body: {body}");
    assert!(body.contains(r#""published":true"#), "body: {body}");
}

/// Positive: the named routes resolve to their expected paths.
#[test]
fn named_example_routes_resolve() {
    let table = super::table();
    assert_eq!(
        table.url("examples.posts.index", &[]).as_deref(),
        Some("/examples/posts")
    );
    assert_eq!(
        table
            .url("examples.posts.show", &[("id", "abc")])
            .as_deref(),
        Some("/examples/posts/abc")
    );
}
