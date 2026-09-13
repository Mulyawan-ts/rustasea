//! Auth routes — login, logout, registration, and password confirmation.
//!
//! `POST /login` and `POST /logout` are implemented:
//!
//! * **Login** throttles first (named `login` limiter), verifies the submitted
//!   credentials against the async [`UserProvider`] seam, and — on success —
//!   sets the hardened session cookie and answers `303 See Other` → the
//!   configured `[fortify].home`.
//! * **Logout** destroys the stored session and clears the session cookie.
//!
//! The remaining POST routes (`/register`, `/confirm-password`,
//! `/email/verification-notification`) still answer `501 Not Implemented`: no
//! flow is faked, so a caller cannot mistake the scaffold for a working
//! authentication surface.
//!
//! [`UserProvider`]: rustasea::auth::UserProvider

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use axum::extract::ConnectInfo;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, Response};

use rustasea::auth::{
    AuthError, Credentials, Guard, Limit, LimiterInput, RateLimiterRegistry, SessionCookieConfig,
    SessionGuard, ThrottleConfig, ThrottleDecision, LOGIN,
};
use rustasea::http::AppState;
use rustasea::router::Router as RouteTable;

use super::helpers::{field, fortify_config, json_error, parse_form, see_other, user_provider};
use super::{not_implemented, session_id_from_headers};

/// Login page markup (placeholder form).
const LOGIN_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Log in</title></head>
<body><main><h1>Log in</h1>
<form method="post" action="/login">
<label>Email <input type="email" name="email" required></label>
<label>Password <input type="password" name="password" required></label>
<button type="submit">Log in</button>
</form>
<p><a href="/register">Create an account</a></p>
</main></body></html>"#;

/// Registration page markup (placeholder form).
const REGISTER_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Register</title></head>
<body><main><h1>Register</h1>
<form method="post" action="/register">
<label>Name <input type="text" name="name" required></label>
<label>Email <input type="email" name="email" required></label>
<label>Password <input type="password" name="password" required></label>
<button type="submit">Register</button>
</form>
<p><a href="/login">Already registered?</a></p>
</main></body></html>"#;

/// Password-confirmation page markup (placeholder form).
const CONFIRM_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Confirm password</title></head>
<body><main><h1>Confirm password</h1>
<form method="post" action="/confirm-password">
<label>Password <input type="password" name="password" required></label>
<button type="submit">Confirm</button>
</form>
</main></body></html>"#;

/// Email-verification notice markup (placeholder).
const VERIFY_EMAIL_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Verify your email</title></head>
<body><main><h1>Verify your email</h1>
<p>We sent a verification link to your email address. Follow it to continue.</p>
<form method="post" action="/email/verification-notification">
<button type="submit">Resend verification email</button>
</form>
</main></body></html>"#;

/// Detail used for both the unknown-email and wrong-password `422` responses.
///
/// The two failure modes are deliberately byte-identical so a caller cannot
/// enumerate accounts by comparing response bodies.
const INVALID_CREDENTIALS: &str = "These credentials do not match our records.";

/// Detail for a `422` when the form omits the username or password.
const MISSING_CREDENTIALS: &str = "The username and password fields are required.";

/// Detail for a `500` when the auth backend cannot be resolved (fail closed).
const AUTH_UNAVAILABLE: &str = "Authentication is temporarily unavailable.";

/// Detail for a `500` when the named `login` limiter is not registered.
const THROTTLE_MISCONFIGURED: &str = "Rate limiting is misconfigured.";

/// Peer address used when neither connection info nor `X-Forwarded-For` is
/// available, so the throttle still gets a finite bucket instead of failing open.
const UNKNOWN_PEER: &str = "0.0.0.0";

/// Register the auth route table onto `table`.
pub fn register(table: &mut RouteTable) {
    table.get_action("/login", login_page).named("login");
    table.post_action("/login", login_submit);
    table.post_action("/logout", logout).named("logout");
    table
        .get_action("/register", register_page)
        .named("register");
    table.post_action("/register", register_submit);
    table
        .get_action("/confirm-password", confirm_page)
        .named("password.confirm");
    table.post_action("/confirm-password", confirm_submit);
    table
        .get_action("/verify-email", verify_email_page)
        .named("verification.notice");
    table.post_action("/email/verification-notification", verify_email_resend);
}

/// GET /login — render the login form.
///
/// The markup is a static string: this scaffold ships no template directory
/// (`resources/views/` has no `login.html`), so the page cannot render a
/// submitted-error message yet. The gap is deliberate and out of scope here —
/// the POST handler still answers with a machine-readable JSON error.
async fn login_page() -> Html<&'static str> {
    Html(LOGIN_HTML)
}

/// POST /login — throttle, verify credentials, set the session cookie.
///
/// # Flow
///
/// 1. Resolve the [`SessionGuard`] from [`AppState`]; absent → `500` (fail
///    closed, never a silent allow).
/// 2. Parse the urlencoded body. The username field is `[fortify].username`
///    (default `email`), so the field name is read from config rather than
///    hardcoded.
/// 3. Throttle via the named `login` limiter, keyed on the normalized username
///    + peer IP. Denied → `429` with `Retry-After`.
/// 4. Verify against the async [`UserProvider`]. On success, set the session
///    cookie and `303 See Other` → `[fortify].home`.
///
/// # Status codes
///
/// * `303` — success; POST → GET redirect so the browser does not re-POST.
/// * `422` — missing fields, or bad credentials (unknown email and wrong
///   password are byte-identical).
/// * `429` — rate limited (`Retry-After` header + JSON body).
/// * `500` — auth backend unavailable or the limiter is misconfigured.
///
/// # Peer IP
///
/// The throttle bucket key is resolved by [`peer_ip`], which honours
/// `X-Forwarded-For` **only** when the deployment declares trusted proxies
/// (`AppState::security.trusted_proxies`; empty by default). With no trusted
/// proxies the connection peer ([`ConnectInfo<SocketAddr>`]) is used verbatim,
/// so a client that is not behind a trusted proxy cannot spoof its bucket.
///
/// [`UserProvider`]: rustasea::auth::UserProvider
async fn login_submit(
    axum::extract::Extension(state): axum::extract::Extension<Arc<AppState>>,
    connect: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(guard) = state.auth::<SessionGuard>() else {
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "AuthError::Unavailable",
            AUTH_UNAVAILABLE,
        );
    };

    let config = fortify_config();
    let fields = parse_form(&body);
    let raw_username = field(&fields, &config.username).unwrap_or_default();
    let password = field(&fields, "password").unwrap_or_default();
    if raw_username.trim().is_empty() || password.is_empty() {
        return json_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "ValidationError",
            MISSING_CREDENTIALS,
        );
    }

    // Apply `lowercase_usernames` before both the throttle key and the lookup,
    // so case variants share one bucket and resolve the same record.
    let username = if config.lowercase_usernames {
        raw_username.trim().to_lowercase()
    } else {
        raw_username.trim().to_string()
    };

    // Throttle before any credential work. The key is derived by the named
    // `login` limiter (`normalized-username|ip`). `peer_ip` honours
    // `X-Forwarded-For` only when the deployment trusts proxies.
    let ip = peer_ip(&headers, connect, &state.security.trusted_proxies);
    let input = LimiterInput::new().with_username(&username).with_ip(&ip);
    match login_registry().check(LOGIN, &input) {
        Ok(ThrottleDecision::Allowed { .. }) => {}
        Ok(ThrottleDecision::Denied { retry_after_secs }) => {
            return throttle_response(retry_after_secs);
        }
        // A missing `login` limiter is a wiring bug: fail closed (500), never
        // an unlimited allow.
        Err(_) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Throttle",
                THROTTLE_MISCONFIGURED,
            );
        }
    }

    let credentials = Credentials {
        email: username,
        password: password.to_string(),
    };
    match guard
        .login_with_provider(user_provider().as_ref(), &credentials)
        .await
    {
        Ok(token) => {
            let cookie = guard.cookie().build_cookie(token.access_token);
            let mut response = see_other(&config.home);
            if let Ok(value) = HeaderValue::try_from(cookie.to_string()) {
                response.headers_mut().append(header::SET_COOKIE, value);
            }
            response
        }
        // Unknown email and wrong password return this exact body.
        Err(AuthError::BadCredentials) => json_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "AuthError::BadCredentials",
            INVALID_CREDENTIALS,
        ),
        Err(_) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "AuthError::Unavailable",
            AUTH_UNAVAILABLE,
        ),
    }
}

/// POST /logout — destroy the session and clear the session cookie.
///
/// The session id is read from the request's session cookie; if present and a
/// guard is wired, [`Guard::logout`] destroys the stored record. Errors are
/// ignored (a missing/forged id, or a store hiccup, must never panic — the
/// cookie is cleared regardless, so the browser ends up logged out).
///
/// # Why there is no `regenerateToken()` step
///
/// Laravel's third logout step, `Session::regenerateToken()`, rotates a
/// session-stored CSRF token. RustaSea has no such token: it uses origin-aware
/// [`PreventRequestForgery`](rustasea::auth::PreventRequestForgery)
/// (`Sec-Fetch-Site` + an allow-list) rather than a per-session CSRF token, so
/// there is nothing to regenerate. A cheap correct analogue would be rotating
/// the session id via [`Guard::refresh`] before clearing the cookie, but that
/// is unnecessary once the record is destroyed and the cookie cleared, so it is
/// intentionally not forced here.
async fn logout(
    axum::extract::Extension(state): axum::extract::Extension<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let guard = state.auth::<SessionGuard>();
    if let (Some(guard), Some(session_id)) = (guard.as_ref(), session_id_from_headers(&headers)) {
        // Ignore the result: teardown is best-effort and must never panic.
        let _ = guard.logout(&session_id).await;
    }

    // Clear the cookie with the same name/path/domain/flags the guard issues it
    // with, so the browser drops exactly the cookie it holds.
    let cookie_config =
        guard.map_or_else(SessionCookieConfig::default, |guard| guard.cookie().clone());
    let mut cookie = cookie_config.build_cookie("");
    cookie.make_removal();

    let mut response = see_other("/");
    if let Ok(value) = HeaderValue::try_from(cookie.to_string()) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

/// GET /register — render the registration form.
async fn register_page() -> Html<&'static str> {
    Html(REGISTER_HTML)
}

/// POST /register — unimplemented registration flow.
async fn register_submit() -> Response {
    not_implemented("Registration")
}

/// GET /confirm-password — render the password-confirmation form.
async fn confirm_page() -> Html<&'static str> {
    Html(CONFIRM_HTML)
}

/// POST /confirm-password — unimplemented confirmation flow.
async fn confirm_submit() -> Response {
    not_implemented("Password confirmation")
}

/// GET /verify-email — render the verification notice.
async fn verify_email_page() -> Html<&'static str> {
    Html(VERIFY_EMAIL_HTML)
}

/// POST /email/verification-notification — unimplemented resend flow.
async fn verify_email_resend() -> Response {
    not_implemented("Verification email resend")
}

/// Process-wide named-limiter registry, built once from `[fortify.limiters]`.
///
/// The registry is shared so its in-memory buckets accumulate across requests
/// (a per-request registry would reset the window and never throttle). It is a
/// module static because the router bootstrap is out of this change's scope and
/// [`AppState`] carries a single type-erased auth slot already used by the
/// guard.
fn login_registry() -> &'static RateLimiterRegistry {
    static REGISTRY: OnceLock<RateLimiterRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| RateLimiterRegistry::from_fortify_config(&fortify_config().limiters))
}

/// Resolve the throttle peer key from connection info and `X-Forwarded-For`.
///
/// # Trust decision
///
/// The app does not hand-roll the proxy decision: it delegates to the crate's
/// single source of truth, [`ThrottleConfig::key_for_peer`], so the login
/// handler and the [`ThrottleLayer`](rustasea::auth::ThrottleLayer) middleware
/// can never disagree about when `X-Forwarded-For` is honoured. Concretely:
///
/// * **No trusted proxies** (`trusted_proxies` empty — the default) →
///   `X-Forwarded-For` is **ignored** and the connection peer wins, so a client
///   that is not behind a trusted proxy cannot spoof its throttle bucket.
/// * **Trusted proxies configured** → the first hop of `X-Forwarded-For` is
///   used, falling back to the connection peer when the header is absent or
///   empty.
///
/// The connection peer is [`ConnectInfo<SocketAddr>`] when the router provides
/// it, otherwise the [`UNKNOWN_PEER`] constant — so the throttle always gets a
/// finite bucket instead of failing open. A deployment behind a proxy that is
/// *not* in `trusted_proxies` must have that proxy strip client-supplied
/// `X-Forwarded-For`, since this handler will ignore the header entirely.
pub(crate) fn peer_ip(
    headers: &HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
    trusted_proxies: &[String],
) -> String {
    let peer = connect
        .map(|ConnectInfo(addr)| addr.ip().to_string())
        .unwrap_or_else(|| UNKNOWN_PEER.to_string());
    let forwarded_for = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok());
    ThrottleConfig::new(Limit::per_minute(1))
        .behind_proxies(trusted_proxies.to_vec())
        .key_for_peer(&peer, forwarded_for)
}

/// Build the `429 Too Many Requests` response with a `Retry-After` header.
fn throttle_response(retry_after_secs: u64) -> Response {
    let mut response = json_error(
        StatusCode::TOO_MANY_REQUESTS,
        "Throttle",
        &format!("Rate limit exceeded; retry in {retry_after_secs} seconds."),
    );
    if let Ok(value) = HeaderValue::try_from(retry_after_secs.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}
