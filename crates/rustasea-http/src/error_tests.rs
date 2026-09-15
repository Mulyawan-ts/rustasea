//! Unit tests for [`crate::error`] — envelope shape, prod redaction, dev page.
//!
//! Split into a sibling file so `error.rs` stays under the 500-line cap.

use super::*;
use axum::body::to_bytes;
use axum::response::IntoResponse;

/// A minimal error type to build a source chain.
#[derive(Debug)]
struct Boom;

impl std::fmt::Display for Boom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "boom-source")
    }
}

impl StdError for Boom {}

/// Read the JSON body out of a response.
async fn body_json(response: Response) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json")
}

/// A `5xx` must hide the message and the source chain from the client.
#[tokio::test]
async fn internal_hides_message_and_source_in_body() {
    let response = AppError::internal_with_source("secret-detail", Box::new(Boom)).into_response();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(response.extensions().get::<ErrorDetail>().is_some());

    let body = body_json(response).await;
    assert_eq!(body["errors"][0]["detail"], "An unexpected error occurred.");
    assert_eq!(body["errors"][0]["code"], "AppError::Internal");
    let rendered = body.to_string();
    assert!(!rendered.contains("secret-detail"));
    assert!(!rendered.contains("boom-source"));
}

/// The marker extension still carries the real diagnostics for the dev path.
#[tokio::test]
async fn internal_marker_carries_diagnostics() {
    let response = AppError::internal_with_source("boom", Box::new(Boom)).into_response();
    let detail = response
        .extensions()
        .get::<ErrorDetail>()
        .expect("marker present");
    assert_eq!(detail.type_name, "AppError::Internal");
    assert_eq!(detail.message, "boom");
    assert_eq!(detail.source_chain, vec!["boom-source".to_string()]);
}

/// A `4xx` keeps its client-facing detail.
#[tokio::test]
async fn validation_keeps_detail() {
    let response = AppError::validation("invalid", "email is required").into_response();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(body["errors"][0]["detail"], "email is required");
    assert_eq!(body["errors"][0]["code"], "invalid");
    assert_eq!(body["errors"][0]["status"], "422");
}

/// `not_found` maps to `404` and keeps its detail.
#[tokio::test]
async fn not_found_maps_to_404() {
    let response = AppError::not_found("no such page").into_response();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = body_json(response).await;
    assert_eq!(body["errors"][0]["detail"], "no such page");
}

/// The envelope always carries a `request_id` key (null before middleware).
#[tokio::test]
async fn envelope_carries_request_id_key() {
    let body = body_json(AppError::internal("x").into_response()).await;
    assert!(body.get("request_id").is_some());
    assert!(body["request_id"].is_null());
}

/// `From<Box<dyn Error>>` maps to `Internal` and keeps the source chain.
#[test]
fn from_boxed_error_is_internal() {
    let boxed: Box<dyn StdError + Send + Sync> = Box::new(Boom);
    let error: AppError = boxed.into();
    assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(error.debug_detail().source_chain, vec!["boom-source"]);
}

/// The dev page escapes HTML in every interpolated value.
#[test]
fn dev_page_escapes_html() {
    let detail = ErrorDetail {
        type_name: "<script>".to_string(),
        message: "<img src=x onerror=alert(1)>".to_string(),
        source_chain: vec!["a & b".to_string()],
    };
    let context = RequestContext {
        method: "GET".to_string(),
        path: "/x".to_string(),
        request_id: Some("<id>".to_string()),
        environment: Some("local".to_string()),
        user: Some("\"quoted\"".to_string()),
        headers: vec![("accept".to_string(), "<text/html>".to_string())],
    };
    let page = render_dev_page(&detail, &context, Some("<code>"));
    assert!(!page.contains("<script>"));
    assert!(!page.contains("<img src=x"));
    assert!(page.contains("&lt;script&gt;"));
    assert!(page.contains("&lt;img src=x onerror=alert(1)&gt;"));
    assert!(page.contains("a &amp; b"));
    assert!(page.contains("&lt;code&gt;"));
}

/// The dev page includes the request context and a snippet when provided.
#[test]
fn dev_page_renders_context_and_snippet() {
    let detail = ErrorDetail::generic("AppError::Panic", "panicked at x");
    let context = RequestContext {
        method: "POST".to_string(),
        path: "/boom".to_string(),
        request_id: Some("abc".to_string()),
        environment: Some("local".to_string()),
        user: None,
        headers: vec![("user-agent".to_string(), "curl".to_string())],
    };
    let page = render_dev_page(&detail, &context, Some("12 | panic!()"));
    assert!(page.contains("panicked at x"));
    assert!(page.contains("POST"));
    assert!(page.contains("/boom"));
    assert!(page.contains("abc"));
    assert!(page.contains("12 | panic!()"));
    assert!(page.contains("curl"));
}

/// `status_title` returns the canonical phrase and a fallback.
#[test]
fn status_titles_are_canonical() {
    assert_eq!(status_title(500), "Internal Server Error");
    assert_eq!(status_title(404), "Not Found");
    assert_eq!(status_title(9999), "Error");
}

/// Long fields are truncated so the page cannot blow up.
#[test]
fn long_fields_are_truncated() {
    let huge = "x".repeat(MAX_FIELD_LEN * 2);
    let detail = ErrorDetail::generic("AppError::Internal", huge.clone());
    let page = render_dev_page(&detail, &RequestContext::default(), None);
    assert!(page.contains("(truncated)"));
    assert!(page.len() < huge.len());
}
