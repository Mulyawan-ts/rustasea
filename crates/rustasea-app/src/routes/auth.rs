//! Auth routes — login, logout, registration, password confirmation, and
//! email verification.
//!
//! `POST /login`, `POST /logout`, and `POST /register` are implemented here.
//! Login throttles via the named `login` limiter, verifies against the async
//! [`UserProvider`] seam, then sets the hardened session cookie and `303` →
//! `[fortify].home`. Logout destroys the stored session and clears the cookie.
//! Registration validates, creates the user, auto-logs them in, and `303` →
//! `[fortify].home`, gated by `[fortify].features.registration` (off → `404`).
//!
//! The password-confirmation flow (AUTH-011) lives in [`confirmation`] and the
//! email-verification flow (AUTH-014) in [`verification`] — both split into
//! submodules to keep this file under the 500-line cap. Every flow is real; no
//! POST route answers a placeholder `501`.
//!
//! [`UserProvider`]: rustasea::auth::UserProvider

/// Password-confirmation flow (AUTH-011).
pub(crate) mod confirmation;
/// Static login/register page markup.
pub(crate) mod pages;
/// Passkeys / WebAuthn flow (AUTH-017).
pub(crate) mod passkeys;
/// Password-reset flow (AUTH-013).
pub(crate) mod password_reset;
/// Two-factor authentication flow (AUTH-016).
pub(crate) mod two_factor;
/// Email-verification flow (AUTH-014).
pub(crate) mod verification;

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use axum::extract::ConnectInfo;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};

use rustasea::auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea::auth::{
    AuthError, Credentials, Guard, Limit, LimiterInput, NewUserRecord, RateLimiterRegistry,
    SessionCookieConfig, SessionGuard, ThrottleConfig, ThrottleDecision, LOGIN,
};
use rustasea::http::AppState;
use rustasea::router::Router as RouteTable;
use rustasea::validation::serde_json::{Map, Value};
use rustasea::validation::{ErrorBag, PasswordPolicy, Rules, ValidationContext, ValidationError};

use super::helpers::{field, fortify_config, json_error, parse_form, see_other, user_provider};
use super::session_id_from_headers;
use pages::{LOGIN_HTML, REGISTER_HTML};

/// Detail used for both the unknown-email and wrong-password `422` responses.
///
/// The two failure modes are deliberately byte-identical so a caller cannot
/// enumerate accounts by comparing response bodies.
const INVALID_CREDENTIALS: &str = "These credentials do not match our records.";

/// Detail for a `422` when the form omits the username or password.
const MISSING_CREDENTIALS: &str = "The username and password fields are required.";

/// Detail for a `500` when the auth backend cannot be resolved (fail closed).
const AUTH_UNAVAILABLE: &str = "Authentication is temporarily unavailable.";

/// Detail for a `500` when registration cannot be hashed or persisted.
const REGISTER_UNAVAILABLE: &str = "Registration is temporarily unavailable.";

/// Detail for the `404` returned when `[fortify].features.registration` is off.
///
/// The kit removes the register routes entirely when the feature is disabled;
/// RustaSea's table is static, so a `404` is the closest honest analogue.
const REGISTRATION_DISABLED: &str = "Registration is disabled.";

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
        .get_action("/confirm-password", confirmation::confirm_page)
        .named("password.confirm");
    table.post_action("/confirm-password", confirmation::confirm_submit);
    table
        .get_action("/verify-email", verification::verify_email_page)
        .named("verification.notice");
    table.post_action(
        "/email/verification-notification",
        verification::verify_email_resend,
    );
    table
        .get_action(
            "/email/verify/{user_id}/{hash}",
            verification::verify_email_confirm,
        )
        .named("verification.verify");
    table
        .get_action("/forgot-password", password_reset::forgot_password_page)
        .named("password.request");
    table
        .post_action("/forgot-password", password_reset::forgot_password_submit)
        .named("password.email");
    table
        .get_action(
            "/reset-password/{token}",
            password_reset::reset_password_page,
        )
        .named("password.reset");
    table
        .post_action("/reset-password", password_reset::reset_password_submit)
        .named("password.store");
    two_factor::register(table);
    passkeys::register(table);
}

/// GET /login — render the login form (static markup; no template dir ships).
async fn login_page() -> Html<&'static str> {
    Html(LOGIN_HTML)
}

/// POST /login — throttle, verify credentials, set the session cookie.
///
/// Resolve the [`SessionGuard`] (`500` if absent); parse the body (username
/// field from `[fortify].username`); throttle via the named `login` limiter
/// (`429` when denied); verify against the async [`UserProvider`], then set the
/// session cookie and `303` → `[fortify].home`. Unknown email and wrong
/// password are byte-identical (`422`). The throttle key is [`peer_ip`], which
/// honours `X-Forwarded-For` only when the deployment trusts proxies.
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

    // Two-factor interception: a confirmed account's login is paused for the
    // challenge; a non-enrolled account falls through to the normal path.
    if let Some(response) = two_factor::intercept_login(&guard, &username, password).await {
        return response;
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
/// The session id is read from the request cookie; if present and a guard is
/// wired, [`Guard::logout`] destroys the stored record. Errors are ignored (a
/// missing/forged id or a store hiccup must never panic — the cookie is cleared
/// regardless). Laravel's `Session::regenerateToken()` has no analogue here:
/// RustaSea uses origin-aware
/// [`PreventRequestForgery`](rustasea::auth::PreventRequestForgery) rather than
/// a per-session CSRF token, so there is nothing to rotate.
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

/// GET /register — render the form (gated like [`register_submit`]).
async fn register_page() -> Response {
    if registration_off() {
        return not_found();
    }
    Html(REGISTER_HTML).into_response()
}

/// POST /register — validate, create, auto-login, `303` home
/// (`CreateNewUser::create` parity).
///
/// Gated by `[fortify].features.registration` (off → `404`; the kit removes the
/// routes, a static table cannot, so `404` is the closest honest analogue).
/// Email is `unique:users,email` with **no** `ignore_id` — any owner conflicts —
/// answered from a pre-awaited [`find_by_email`](UserProvider::find_by_email)
/// probe (the sync [`ValidationContext`] cannot await). Failure → `422`
/// [`ErrorBag`]; success → hash, [`UserProvider::create`] with
/// `email_verified_at = None` (a fresh user lands on `/verify-email`),
/// auto-login, cookie, `303` → `[fortify].home`. A persisted race
/// ([`AuthError::UserExists`]) → `422` `email`. No `register` limiter (kit
/// parity). [`UserProvider`]: rustasea::auth::UserProvider
async fn register_submit(
    axum::extract::Extension(state): axum::extract::Extension<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Response {
    if registration_off() {
        return not_found();
    }
    let Some(guard) = state.auth::<SessionGuard>() else {
        return fail_closed(AUTH_UNAVAILABLE);
    };
    let config = fortify_config();
    let fields = parse_form(&body);
    let name = field(&fields, "name").unwrap_or_default().to_string();
    let raw_email = field(&fields, "email").unwrap_or_default().trim();
    let email = if config.lowercase_usernames {
        raw_email.to_lowercase()
    } else {
        raw_email.to_string()
    };
    let password = field(&fields, "password").unwrap_or_default().to_string();

    // A missing field is an absent key (fails `required`), not `""`.
    let mut payload = Map::new();
    for key in ["name", "password", "password_confirmation"] {
        if let Some(value) = field(&fields, key) {
            payload.insert(key.to_string(), Value::String(value.to_string()));
        }
    }
    if field(&fields, "email").is_some() {
        payload.insert("email".to_string(), Value::String(email.clone()));
    }
    // Pre-await the one uniqueness probe the sync context cannot await; a
    // lookup error becomes `None` (unavailable), which `is_unique` fails closed.
    let provider = user_provider();
    let context = RegistrationContext {
        queried_email: email.clone(),
        unique: provider
            .find_by_email(&email)
            .await
            .ok()
            .map(|found| found.is_none()),
    };
    let rules = Rules::new()
        .password_policy(PasswordPolicy::production())
        .field("name", "required|string|max:255")
        .field("email", "required|string|email|max:255|unique:users,email")
        .field("password", "required|string|password|confirmed");
    if let Err(bag) = rules.validate_with(&Value::Object(payload), &context) {
        return validation_error(bag);
    }
    let Ok(hash) = Argon2Verifier::new().hash(&password) else {
        return fail_closed(REGISTER_UNAVAILABLE);
    };
    match provider
        .create(NewUserRecord::new(name, email.clone(), hash))
        .await
    {
        Ok(_) => {}
        // A collision the probe missed (a concurrent write): same shape as `unique`.
        Err(AuthError::UserExists { .. }) => {
            return field_error("email", "unique", "The email has already been taken.");
        }
        Err(_) => return fail_closed(REGISTER_UNAVAILABLE),
    }
    let credentials = Credentials { email, password };
    match guard
        .login_with_provider(provider.as_ref(), &credentials)
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
        Err(_) => fail_closed(REGISTER_UNAVAILABLE),
    }
}

/// `500` fail-closed response.
fn fail_closed(detail: &str) -> Response {
    json_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "AuthError::Unavailable",
        detail,
    )
}

/// Whether registration is disabled (test override included).
fn registration_off() -> bool {
    #[cfg(test)]
    if REGISTRATION_OFF.load(std::sync::atomic::Ordering::SeqCst) {
        return true;
    }
    !fortify_config().features.registration
}

/// Test-only registration-disable flag; see [`set_registration_disabled`].
#[cfg(test)]
static REGISTRATION_OFF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Force the registration feature gate off (tests serialize via the lock).
#[cfg(test)]
pub(crate) fn set_registration_disabled(off: bool) {
    REGISTRATION_OFF.store(off, std::sync::atomic::Ordering::SeqCst);
}

/// `404` returned when registration is disabled.
fn not_found() -> Response {
    json_error(
        StatusCode::NOT_FOUND,
        "RegistrationDisabled",
        REGISTRATION_DISABLED,
    )
}

/// `422` carrying an [`ErrorBag`] in the documented shape (`api-validation.md`).
fn validation_error(bag: ErrorBag) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        axum::Json(bag.into_json_body()),
    )
        .into_response()
}

/// `422` with a single field error in the [`validation_error`] shape.
fn field_error(field: &str, code: &str, message: &str) -> Response {
    let mut bag = ErrorBag::new();
    bag.add(field, ValidationError::new(code, message));
    validation_error(bag)
}

/// [`ValidationContext`] adapter over the pre-awaited registration probe.
/// Registration passes **no** `ignore_id`, so **any** existing owner conflicts.
/// Fail-closed: an unavailable probe or a mismatched value yields `None`.
struct RegistrationContext {
    /// The email the probe was resolved for; other values fail closed.
    queried_email: String,
    /// `Some(true)` = free, `Some(false)` = owned, `None` = unavailable.
    unique: Option<bool>,
}

impl ValidationContext for RegistrationContext {
    fn is_unique(
        &self,
        table: &str,
        column: &str,
        value: &str,
        _ignore_id: Option<&str>,
    ) -> Option<bool> {
        (table == "users" && column == "email" && value == self.queried_email)
            .then_some(self.unique)
            .flatten()
    }
}

/// Process-wide named-limiter registry, built once from `[fortify.limiters]`.
/// Shared so its in-memory buckets accumulate across requests; a per-request
/// registry would reset the window and never throttle.
fn login_registry() -> &'static RateLimiterRegistry {
    static REGISTRY: OnceLock<RateLimiterRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| RateLimiterRegistry::from_fortify_config(&fortify_config().limiters))
}

/// Resolve the throttle peer key from connection info and `X-Forwarded-For`.
///
/// Delegates to [`ThrottleConfig::key_for_peer`]: with no trusted proxies
/// `X-Forwarded-For` is ignored and the connection peer wins; with trusted
/// proxies the first forwarded hop is used, falling back to the peer.
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
