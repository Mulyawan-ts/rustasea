//! Request-error middleware — dev page + prod JSON envelope (ADOPT-010).
//!
//! [`error_middleware`] sits **inside** the session layer (so the
//! `Extension<AuthUser>` projection is already present) and **outside** the
//! panic-catch layer (so a caught panic arrives here as an ordinary `5xx`). It
//! inspects every response:
//!
//! * A `5xx` is rewritten. In debug mode it becomes a rich HTML error page (or a
//!   rich JSON envelope when the client prefers JSON); in production it becomes
//!   the generic JSON envelope — never leaking the message, source chain, or a
//!   backtrace.
//! * Any response gets a resolved `x-request-id` header (the incoming value, or
//!   a generated v7 UUID when absent), so logs and the client share an id.
//!
//! # Why capture state by value
//!
//! The `Extension<Arc<AppState>>` layer installed by
//! [`compile`](super::compile) is **innermost**, so it has not run when this
//! middleware executes. The debug flag and environment are therefore captured
//! at layer-construction time (the same pattern [`with_debugbar`](super) uses),
//! not read from the request.

use axum::extract::Request;
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use rustasea::auth::AuthUser;
use rustasea::http::error::{
    render_dev_page, render_error_json, status_title, ErrorDetail, RequestContext,
};
use rustasea::http::AppState;

/// Header carrying the resolved request id.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Generic prod detail for every `5xx` (never the real message).
const GENERIC_DETAIL: &str = "An unexpected error occurred.";

/// Headers copied into the dev page context (never `authorization`/`cookie`).
const CONTEXT_HEADERS: [&str; 4] = ["content-type", "accept", "user-agent", "x-request-id"];

/// Build the error middleware closure and apply it to `router`.
///
/// Captures `state.debug` and `state.env` by value, mirroring the
/// [`with_debugbar`](super::with_debugbar) construction pattern.
pub fn with_errors(router: axum::Router, state: &AppState) -> axum::Router {
    let debug = state.debug;
    let environment = state.env.clone();
    router.layer(axum::middleware::from_fn(move |request, next| {
        let environment = environment.clone();
        async move { error_middleware(debug, Some(environment), request, next).await }
    }))
}

/// Apply the panic-catch layer to a compiled router.
///
/// Wraps the core router so a panic anywhere inside the route stack (including
/// in a handler) becomes an `AppError::Panic` `500` response that
/// [`error_middleware`] then renders. It is the innermost layer of the chain.
pub fn with_panic_catch(router: axum::Router) -> axum::Router {
    router.layer(rustasea_http::panic::catch_panic_layer())
}

/// Inspect a response and render a `5xx` as a dev page or prod envelope.
///
/// The request id is resolved once (incoming header or generated v7 UUID) and
/// echoed on the response. Non-`5xx` responses pass through unchanged apart
/// from that header.
pub async fn error_middleware(
    debug: bool,
    environment: Option<String>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let request_id = resolve_request_id(request.headers());
    let user = request.extensions().get::<AuthUser>().map(describe_user);
    let context_headers = whitelisted_headers(request.headers());

    let mut response = next.run(request).await;

    if response.status().is_server_error() {
        let status = response.status();
        let detail = response
            .extensions()
            .get::<ErrorDetail>()
            .cloned()
            .unwrap_or_else(|| {
                ErrorDetail::generic("InternalServerError", format!("{}", status.as_u16()))
            });

        let context = RequestContext {
            method: method.to_string(),
            path,
            request_id: Some(request_id.clone()),
            environment,
            user,
            headers: context_headers,
        };

        response = render_server_error(debug, status, &detail, &context, &request_id);
    }

    set_request_id(&mut response, &request_id);
    response
}

/// Resolve the request id: the incoming header, or a generated v7 UUID.
fn resolve_request_id(headers: &HeaderMap) -> String {
    headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string())
}

/// Render a `5xx` into the dev or prod response.
fn render_server_error(
    debug: bool,
    status: StatusCode,
    detail: &ErrorDetail,
    context: &RequestContext,
    request_id: &str,
) -> Response {
    if !debug {
        // Production: generic envelope, no message/source/backtrace.
        let title = status_title(status.as_u16());
        let body = render_error_json(
            status.as_u16(),
            &detail.type_name,
            title,
            GENERIC_DETAIL,
            Some(request_id),
        );
        return (status, axum::Json(body)).into_response();
    }

    if prefers_json(&context.headers) {
        // Dev + JSON client: the rich message/source chain is safe in debug.
        let title = status_title(status.as_u16());
        let body = render_error_json(
            status.as_u16(),
            &detail.type_name,
            title,
            &detail.message,
            Some(request_id),
        );
        return (status, axum::Json(body)).into_response();
    }

    // Dev + browser: full HTML page with cause chain and (panic) snippet.
    let snippet = panic_snippet(detail);
    let page = render_dev_page(detail, context, snippet.as_deref());
    (
        status,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )],
        page,
    )
        .into_response()
}

/// Whether the request's `accept` header prefers JSON over HTML.
///
/// A browser sends `text/html` first; an API client sends `application/json`.
/// When both or neither appear, HTML is the dev default.
fn prefers_json(headers: &[(String, String)]) -> bool {
    let accept = headers
        .iter()
        .find(|(name, _)| name == "accept")
        .map(|(_, value)| value.to_ascii_lowercase())
        .unwrap_or_default();
    accept.contains("application/json") && !accept.contains("text/html")
}

/// Build a source snippet for a panic error, when a location was recorded.
///
/// Only panics carry a useful snippet; the location comes from the process-wide
/// hook installed at boot ([`rustasea_http::panic::install_panic_location_hook`]).
fn panic_snippet(detail: &ErrorDetail) -> Option<String> {
    if !detail.type_name.contains("Panic") {
        return None;
    }
    let location = rustasea_http::panic::last_panic_location()?;
    if location.file.is_empty() || location.line == 0 {
        return None;
    }
    rustasea_http::panic::source_snippet(&location.file, location.line, 3)
}

/// Describe the authenticated principal as `id` or `id <email>`.
fn describe_user(user: &AuthUser) -> String {
    match &user.email {
        Some(email) => format!("{} <{}>", user.id, email),
        None => user.id.clone(),
    }
}

/// Collect the whitelisted request headers for the dev page.
///
/// Only [`CONTEXT_HEADERS`] are copied; `authorization` and `cookie` are never
/// included, so the page cannot leak a bearer token or a session id.
fn whitelisted_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for name in CONTEXT_HEADERS {
        if let Some(value) = headers.get(name).and_then(|value| value.to_str().ok()) {
            out.push((name.to_string(), value.to_string()));
        }
    }
    out
}

/// Set the `x-request-id` header on a response.
fn set_request_id(response: &mut Response, request_id: &str) {
    let name = HeaderName::from_static(REQUEST_ID_HEADER);
    if let Ok(value) = HeaderValue::from_str(request_id) {
        response.headers_mut().insert(name, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A generated id is a non-empty UUID when no header is present.
    #[test]
    fn request_id_is_generated_when_absent() {
        let id = resolve_request_id(&HeaderMap::new());
        assert!(!id.is_empty());
        assert!(uuid::Uuid::parse_str(&id).is_ok());
    }

    /// An incoming id is echoed verbatim.
    #[test]
    fn request_id_echoes_incoming_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static(REQUEST_ID_HEADER),
            HeaderValue::from_static("abc"),
        );
        assert_eq!(resolve_request_id(&headers), "abc");
    }

    /// JSON is preferred only when it appears without `text/html`.
    #[test]
    fn json_preference_is_detected() {
        let json = vec![("accept".to_string(), "application/json".to_string())];
        assert!(prefers_json(&json));
        let html = vec![("accept".to_string(), "text/html".to_string())];
        assert!(!prefers_json(&html));
        let both = vec![(
            "accept".to_string(),
            "text/html,application/json".to_string(),
        )];
        assert!(!prefers_json(&both));
        assert!(!prefers_json(&[]));
    }

    /// Only whitelisted headers are collected; secrets are dropped.
    #[test]
    fn whitelist_excludes_secrets() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer SECRET"),
        );
        headers.insert(header::COOKIE, HeaderValue::from_static("session=SECRET"));
        headers.insert(header::USER_AGENT, HeaderValue::from_static("curl"));
        let collected = whitelisted_headers(&headers);
        let joined = format!("{collected:?}");
        assert!(joined.contains("curl"));
        assert!(!joined.contains("SECRET"));
    }
}
