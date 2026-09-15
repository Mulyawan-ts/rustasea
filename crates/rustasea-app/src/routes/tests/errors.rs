//! Error-middleware tests — dev page, prod envelope, and no-secret-leak (ADOPT-010).
//!
//! Every case drives a **fully compiled** router (`compile(table(), state)`), so
//! the real layer chain — panic catch, error middleware, session, CSRF, Sentry,
//! debugbar — is exercised, not a hand-built stack. Split into a sibling module
//! so `tests.rs` stays under the 500-line cap.

use std::error::Error as StdError;
use std::sync::Arc;

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use rustasea::http::error::AppError;
use rustasea::http::AppState;
use rustasea::router::Router as RouteTable;

use super::{call, get};
use crate::routes::{compile, table};

/// A panic raised inside a handler, to prove the catch layer renders it.
async fn panic_handler() -> Response {
    panic!("kaboom-message");
}

/// A source error to build a chain for the `AppError` handler.
#[derive(Debug)]
struct SourceError;

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "root-cause-text")
    }
}

impl StdError for SourceError {}

/// A handler returning a typed `AppError` with a source chain.
async fn app_error_handler() -> Response {
    AppError::internal_with_source("boom-message", Box::new(SourceError)).into_response()
}

/// A handler returning a bare `500` with no typed error attached.
async fn bare_500_handler() -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

/// Build a router with the error test routes and the given debug flag.
///
/// Registered through the real `compile()` path so the full middleware chain
/// (including `with_errors`) is present.
fn router_with_debug(debug: bool) -> axum::Router {
    let mut table: RouteTable = table();
    table.get_action("/__test/panic", panic_handler);
    table.get_action("/__test/app-error", app_error_handler);
    table.get_action("/__test/bare-500", bare_500_handler);
    compile(table, Arc::new(AppState::new("testing", debug)))
}

/// Read the `x-request-id` header value, if present.
fn request_id(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// Dev + panic → 500 HTML page with the message, context, and a generated id.
#[tokio::test]
async fn dev_panic_renders_html_page() {
    let (status, headers, body) = call(router_with_debug(true), get("/__test/panic")).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html")),
        "dev page must be HTML"
    );
    assert!(body.contains("kaboom-message"), "body: {body}");
    assert!(
        body.contains("/__test/panic"),
        "context path missing: {body}"
    );
    assert!(body.contains("Request ID"), "context section missing");
    assert!(
        request_id(&headers).is_some(),
        "request id header must be set"
    );
}

/// Dev + `AppError` → HTML page with the message and the source chain.
#[tokio::test]
async fn dev_app_error_renders_cause_chain() {
    let (status, headers, body) = call(router_with_debug(true), get("/__test/app-error")).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body.contains("boom-message"), "body: {body}");
    assert!(
        body.contains("root-cause-text"),
        "cause chain missing: {body}"
    );
    assert!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html")),
        "dev page must be HTML"
    );
}

/// Dev + `Accept: application/json` → JSON envelope with the real detail.
#[tokio::test]
async fn dev_json_client_gets_rich_envelope() {
    let mut request = get("/__test/app-error");
    request
        .headers_mut()
        .insert(header::ACCEPT, HeaderValue::from_static("application/json"));

    let (status, headers, body) = call(router_with_debug(true), request).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(parsed["errors"][0]["detail"], "boom-message");
    assert!(parsed["request_id"].is_string(), "body: {body}");
}

/// Prod + `AppError` → generic envelope; message and source never leak.
#[tokio::test]
async fn prod_app_error_hides_diagnostics() {
    let (status, headers, body) = call(router_with_debug(false), get("/__test/app-error")).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(
        parsed["errors"][0]["detail"],
        "An unexpected error occurred."
    );
    assert!(parsed["request_id"].is_string(), "body: {body}");
    assert!(!body.contains("boom-message"), "message leaked: {body}");
    assert!(!body.contains("root-cause-text"), "source leaked: {body}");
    assert!(!body.contains("panicked"), "body: {body}");
}

/// Prod + bare `500` → generic envelope with a request id.
#[tokio::test]
async fn prod_bare_500_gets_envelope() {
    let (status, _, body) = call(router_with_debug(false), get("/__test/bare-500")).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(
        parsed["errors"][0]["detail"],
        "An unexpected error occurred."
    );
    assert!(parsed["request_id"].is_string(), "body: {body}");
}

/// An incoming `x-request-id` is echoed and carried in the envelope.
#[tokio::test]
async fn incoming_request_id_is_echoed() {
    let mut request = get("/__test/bare-500");
    request
        .headers_mut()
        .insert("x-request-id", HeaderValue::from_static("abc"));

    let (status, headers, body) = call(router_with_debug(false), request).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(request_id(&headers).as_deref(), Some("abc"));
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(parsed["request_id"], "abc");
}

/// Dev 500 with secret request headers → the page never leaks them.
#[tokio::test]
async fn dev_page_never_leaks_secret_headers() {
    let mut request = get("/__test/bare-500");
    request.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer SECRET"),
    );
    request
        .headers_mut()
        .insert(header::COOKIE, HeaderValue::from_static("session=SECRET"));

    let (status, _, body) = call(router_with_debug(true), request).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        !body.contains("SECRET"),
        "a secret request header leaked into the dev page: {body}"
    );
}

/// The error middleware also stamps `x-request-id` on non-5xx responses.
#[tokio::test]
async fn success_response_gets_request_id_header() {
    let (status, headers, _) = call(router_with_debug(true), get("/health")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        request_id(&headers).is_some(),
        "every response must carry an x-request-id"
    );
}
