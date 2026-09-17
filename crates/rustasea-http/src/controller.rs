//! Base `Controller` trait and standalone redirect helpers.
//!
//! This module adopts the Hypervel/Laravel base-controller convention: an
//! `app/Http/Controllers/Controller.php` superclass that every application
//! controller extends to inherit a small set of shared response helpers.
//! Hypervel 0.4 ships that base class with the `AuthorizesRequests`,
//! `ValidatesRequests`, and response traits composed onto it; the RustaSea
//! equivalent keeps the response surface as a default-method trait so a
//! controller type opts in with `impl Controller for MyController {}`.
//!
//! ADOPT intent: the framework owns the redirect and JSON response helpers once,
//! so application code stops duplicating them. The trait methods are static
//! (no `self`) and delegate to the free functions or to
//! [`crate::JsonResponse`], so a trait impl is a zero-cost marker and the free
//! functions remain callable on their own.
//!
//! Reference: hypervel/hypervel 0.4 `app/Http/Controllers/Controller.php`.

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::Value;

use crate::JsonResponse;

/// Build a `302 Found` redirect to `location`.
///
/// `302` is the plain "moved temporarily" response. An invalid location (one
/// that cannot be encoded as a header value, for example one containing a
/// control character) falls back to `/` rather than panicking, so a hostile or
/// malformed target can never crash the handler.
pub fn redirect(location: &str) -> Response {
    let value = HeaderValue::try_from(location).unwrap_or_else(|_| HeaderValue::from_static("/"));
    (StatusCode::FOUND, [(header::LOCATION, value)]).into_response()
}

/// Build a `303 See Other` redirect to `location`.
///
/// `303` (not `302`) converts the request method to `GET`, so a browser that
/// follows the redirect after a `POST` does not re-submit the body (for
/// example, credentials on a login form). An invalid location falls back to `/`
/// rather than panicking.
pub fn see_other(location: &str) -> Response {
    let value = HeaderValue::try_from(location).unwrap_or_else(|_| HeaderValue::from_static("/"));
    (StatusCode::SEE_OTHER, [(header::LOCATION, value)]).into_response()
}

/// Base controller trait: shared response helpers inherited by controllers.
///
/// This is the RustaSea analogue of Hypervel's `app/Http/Controllers/Controller`
/// base class. Every method is a static default that delegates to the free
/// redirect functions or to [`crate::JsonResponse`], so an application
/// controller opts in with an empty impl:
///
/// ```rust
/// use rustasea_http::Controller;
///
/// struct HomeController;
/// impl Controller for HomeController {}
///
/// let response = HomeController::redirect("/dashboard");
/// # let _ = response;
/// ```
///
/// The trait carries no state and no `self`, so it composes cleanly with the
/// `Send + Sync + 'static` bounds a handler needs.
pub trait Controller: Send + Sync + 'static {
    /// Build a `200 OK` JSON response from a serializable value.
    fn json<T: Serialize>(data: T) -> Response {
        JsonResponse::ok(data)
    }

    /// Build a JSON response with an explicit status code.
    fn json_status<T: Serialize>(status: StatusCode, data: T) -> Response {
        JsonResponse::with_status(status, data)
    }

    /// Build a `422 Unprocessable Entity` validation error response.
    fn validation_error(errors: Value) -> Response {
        JsonResponse::validation_error(errors)
    }

    /// Build a `302 Found` redirect (see [`redirect`]).
    fn redirect(location: &str) -> Response {
        self::redirect(location)
    }

    /// Build a `303 See Other` redirect (see [`see_other`]).
    fn see_other(location: &str) -> Response {
        self::see_other(location)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Minimal controller used to exercise every trait default.
    struct TestController;

    impl Controller for TestController {}

    #[test]
    fn redirect_is_found_with_location() {
        let response = redirect("/dashboard");
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(
            response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some("/dashboard")
        );
    }

    #[test]
    fn see_other_is_see_other_with_location() {
        let response = see_other("/login");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some("/login")
        );
    }

    #[test]
    fn redirect_falls_back_on_invalid_header() {
        let response = redirect("bad\nvalue");
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(
            response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some("/")
        );
    }

    #[test]
    fn see_other_falls_back_on_invalid_header() {
        let response = see_other("bad\nvalue");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some("/")
        );
    }

    #[test]
    fn trait_helpers_delegate_to_free_functions() {
        let found = TestController::redirect("/dashboard");
        assert_eq!(found.status(), StatusCode::FOUND);

        let other = TestController::see_other("/login");
        assert_eq!(other.status(), StatusCode::SEE_OTHER);
    }

    #[test]
    fn trait_json_helpers_set_status_and_content_type() {
        let ok = TestController::json(json!({"ok": true}));
        assert_eq!(ok.status(), StatusCode::OK);
        assert_eq!(
            ok.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );

        let created = TestController::json_status(StatusCode::CREATED, json!({"id": 1}));
        assert_eq!(created.status(), StatusCode::CREATED);
        assert_eq!(
            created
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );

        let invalid = TestController::validation_error(json!({"email": ["required"]}));
        assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            invalid
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
    }
}
