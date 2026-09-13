//! Redirect registration helpers.
//!
//! [`Router::redirect`] and [`Router::redirect_permanent`] register a `GET`
//! route that answers with an HTTP redirect to a literal target. The status
//! codes follow Laravel's `Route::redirect` (302 Found) and
//! `Route::permanentRedirect` (301 Moved Permanently) — axum's own
//! [`axum::response::Redirect`] maps to 303/308, so the response is emitted
//! directly from a status code plus a `Location` header to preserve the
//! expected numeric codes.

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::handler::BoundAction;
use crate::router::Router;

/// Which redirect status code a helper emits.
#[derive(Clone, Copy)]
enum RedirectKind {
    /// `302 Found` — temporary.
    Temporary,
    /// `301 Moved Permanently` — permanent.
    Permanent,
}

impl RedirectKind {
    /// The HTTP status code for this kind.
    fn status(self) -> StatusCode {
        match self {
            Self::Temporary => StatusCode::FOUND,
            Self::Permanent => StatusCode::MOVED_PERMANENTLY,
        }
    }
}

impl Router {
    /// Register a `GET` route that redirects to `to` with `302 Found`.
    ///
    /// The route is introspectable like any other (call [`Router::named`] to
    /// name it) and dispatches a `Location` header pointing at the literal
    /// `to` target.
    pub fn redirect(&mut self, from: &str, to: &str) -> &mut Self {
        self.push_redirect(from, to, RedirectKind::Temporary)
    }

    /// Register a `GET` route that redirects to `to` with `301 Moved
    /// Permanently`.
    pub fn redirect_permanent(&mut self, from: &str, to: &str) -> &mut Self {
        self.push_redirect(from, to, RedirectKind::Permanent)
    }

    /// Register the redirect route and its bound action.
    fn push_redirect(&mut self, from: &str, to: &str, kind: RedirectKind) -> &mut Self {
        self.begin_batch();
        self.push("GET", from);
        if let Some(entry) = self.routes.last_mut() {
            // A redirect is self-contained — drop any pending controller ref so
            // resolution dispatches the redirect handler, not a stub action.
            entry.controller = None;
            entry.handler = Some(format!("redirect:{to}"));
        }
        let full = self
            .routes
            .last()
            .expect("push always appends a route entry")
            .path
            .clone();
        let target = to.to_string();
        let status = kind.status();
        let router = axum::routing::get(move || {
            let target = target.clone();
            async move { redirect_response(status, &target) }
        });
        self.actions.push(BoundAction {
            method: "GET".to_string(),
            path: full,
            domain: self.pending_domain.clone(),
            router,
        });
        self
    }
}

/// Build the redirect response — status code plus `Location` header.
///
/// Falls back to a `500` when `target` is not a valid header value rather than
/// panicking, so a malformed target degrades to an error response.
fn redirect_response(status: StatusCode, target: &str) -> Response {
    match header::HeaderValue::try_from(target) {
        Ok(location) => (status, [(header::LOCATION, location)]).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
