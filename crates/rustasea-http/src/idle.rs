//! Idle (inter-chunk) timeout enforcement for buffered HTTP client responses.
//!
//! The plain send path returns the `reqwest::Response` untouched, so an idle
//! timeout can only be surfaced as a typed [`HttpError`] if the body is read
//! inside [`crate::HttpClient::send_with`]. When `idle_timeout` is set the
//! response body is therefore drained chunk by chunk here, each chunk guarded
//! by a `tokio::time::timeout`, and the response is rebuilt around the buffered
//! bytes. When `idle_timeout` is unset the caller keeps the untouched response
//! and this module is never entered.

use std::time::Duration;

use crate::{HttpError, TimeoutKind};

/// Drain a response body into memory, aborting with [`HttpError::Timeout`]
/// carrying [`TimeoutKind::Idle`] when the gap between consecutive chunks
/// exceeds `idle`.
///
/// The timer resets on every received chunk, so a slow but steady stream is not
/// penalised; only an inter-chunk silence longer than `idle` aborts. On success
/// the response is rebuilt around the buffered body, preserving status,
/// version, headers, extensions, and the request URL.
pub(crate) async fn buffer_with_idle_timeout(
    mut response: reqwest::Response,
    idle: Duration,
) -> Result<reqwest::Response, HttpError> {
    let mut body = Vec::new();
    loop {
        match tokio::time::timeout(idle, response.chunk()).await {
            Ok(Ok(Some(chunk))) => body.extend_from_slice(&chunk),
            Ok(Ok(None)) => break,
            Ok(Err(source)) => return Err(crate::map_timeout(source)),
            Err(_elapsed) => {
                return Err(HttpError::Timeout {
                    kind: TimeoutKind::Idle,
                })
            }
        }
    }
    Ok(rebuild(response, body))
}

/// Rebuild a response whose body has been buffered, preserving the response
/// metadata that callers observe.
fn rebuild(response: reqwest::Response, body: Vec<u8>) -> reqwest::Response {
    use reqwest::ResponseBuilderExt;

    let url = response.url().clone();
    let (mut parts, _streamed) = http::Response::<reqwest::Body>::from(response).into_parts();

    // The async client stores the URL in a private field rather than the
    // extension map, so a plain `http::Response` round-trip drops it. Re-attach
    // it through the public builder extension trait so `Response::url()`
    // survives the rebuild.
    if let Some(extensions) = http::Response::builder().url(url).extensions_mut() {
        parts.extensions.extend(std::mem::take(extensions));
    }

    http::Response::from_parts(parts, reqwest::Body::from(body)).into()
}
