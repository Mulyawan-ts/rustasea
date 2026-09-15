//! Two-factor authentication routes (AUTH-016) — management + login challenge.
//!
//! Split out of [`super`] (held at the 500-line cap). Two surfaces:
//!
//! * **Management** (behind the `password.confirm` gate, Fortify's
//!   `confirmPassword` parity): `POST /user/two-factor-authentication` enables
//!   (returns the secret + `otpauth://` URI), `POST
//!   /user/confirmed-two-factor-authentication` confirms with a code, `DELETE
//!   /user/two-factor-authentication` disables, and `GET`/`POST
//!   /user/two-factor-recovery-codes` view/regenerate the codes.
//! * **Challenge**: `GET`/`POST /two-factor-challenge`. A confirmed user's
//!   login is interrupted by [`intercept_login`], which stores a pending
//!   identity under a dedicated session key and `302`s to the challenge. The
//!   POST verifies a TOTP code or a recovery code under the `two-factor`
//!   limiter, then promotes the pending identity into a real session.
//!
//! # Why a separate pending key
//!
//! The pending identity is written under `<prefix>two_factor_pending`, not the
//! guard's `<prefix>user` key, so the session middleware's `guard.parse` does
//! **not** authenticate it — the user remains a guest until the challenge
//! succeeds, exactly as `AuthenticationTest` asserts.
//!
//! # Secrets
//!
//! Secrets are sealed with [`SecretCipher`] before they reach the store and are
//! never logged. Recovery codes are stored as Argon2 hashes; their plaintext is
//! cached only for the GET view (see [`wiring`]).
//!
//! [`SecretCipher`]: rustasea::auth::two_factor::SecretCipher

pub(crate) mod wiring;

use std::sync::{Arc, OnceLock};

use axum::extract::Extension;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use tower_sessions::session::{Id, Session};

use rustasea::auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea::auth::{
    AuthError, AuthUser, LimiterInput, SessionGuard, SessionUser, ThrottleDecision, TWO_FACTOR,
};
use rustasea::http::AppState;
use rustasea::router::Router as RouteTable;
use rustasea::view::{MinijinjaEngine, ViewEngine, ViewError, ViewResponse};

use crate::routes::helpers::{
    field, fortify_config, json_error, parse_form, see_other, user_provider,
};
use crate::routes::{session_id_from_headers, LOGIN_PATH, PASSWORD_CONFIRM};

use super::{fail_closed, throttle_response, AUTH_UNAVAILABLE, INVALID_CREDENTIALS};
use wiring::{
    cached_recovery_codes, clear_recovery_codes, store_recovery_codes, two_factor_registry,
    two_factor_service, two_factor_store,
};

/// The challenge endpoint (GET renders, POST verifies).
pub(crate) const CHALLENGE_PATH: &str = "/two-factor-challenge";

/// Detail for a `422` when a TOTP or recovery code does not verify.
const INVALID_CODE: &str = "The provided two-factor code was invalid.";

/// Detail for a `409` when recovery codes are regenerated without a secret.
const TWO_FACTOR_NOT_ENABLED: &str = "Two-factor authentication is not enabled.";

/// Detail for a `404` when the plaintext codes are no longer displayable.
const RECOVERY_CODES_UNAVAILABLE: &str =
    "Recovery codes are no longer displayable; regenerate a new set.";

/// Register the two-factor route table onto `table`.
///
/// The management routes live inside a [`RouteTable::group`] because
/// [`RouteTable::middleware`] is sticky — the group scopes the
/// `password.confirm` gate so it cannot leak onto later routes. The challenge
/// routes stay ungated (the user is a guest until the challenge succeeds).
pub(crate) fn register(table: &mut RouteTable) {
    table
        .get_action(CHALLENGE_PATH, challenge_page)
        .named("two-factor.login");
    table.post_action(CHALLENGE_PATH, challenge_submit);

    table.group(|group| {
        group
            .middleware(PASSWORD_CONFIRM)
            .post_action("/user/two-factor-authentication", enable)
            .post_action("/user/confirmed-two-factor-authentication", confirm)
            .delete_action("/user/two-factor-authentication", disable)
            .get_action("/user/two-factor-recovery-codes", recovery_codes_show)
            .post_action("/user/two-factor-recovery-codes", recovery_codes_regenerate);
    });
}

/// JSON body for the enable response (the one-time secret + provisioning URI).
#[derive(Serialize)]
struct EnrollmentResponse {
    /// Plaintext base32 secret to enter/scan once.
    secret: String,
    /// `otpauth://` provisioning URI for the authenticator app.
    otpauth_uri: String,
    /// Plaintext recovery codes (shown once; only hashes are stored).
    recovery_codes: Vec<String>,
}

/// JSON body confirming two-factor authentication.
#[derive(Serialize)]
struct ConfirmedResponse {
    /// Always `true` on success.
    confirmed: bool,
}

/// JSON body for the recovery-code view/regenerate responses.
#[derive(Serialize)]
struct RecoveryCodesResponse {
    /// Plaintext recovery codes (shown once; only hashes are stored).
    recovery_codes: Vec<String>,
}

/// Interrupt a login when the account has confirmed two-factor authentication.
///
/// Returns `Some(response)` when the caller must **not** complete the login:
/// either the challenge was started (`302` → [`CHALLENGE_PATH`]) or the
/// password was wrong (`422`, byte-identical to the normal failure). Returns
/// `None` when the caller should proceed with the ordinary login path (no
/// account, or an account without confirmed two-factor).
///
/// Fail-closed: a store error while checking the enrollment is `500`, never a
/// silent bypass of the second factor.
pub(crate) async fn intercept_login(
    guard: &SessionGuard,
    username: &str,
    password: &str,
) -> Option<Response> {
    let record = match user_provider().find_by_email(username).await {
        Ok(Some(record)) => record,
        // Unknown user: the normal path answers BadCredentials.
        Ok(None) => return None,
        // Provider hiccup: let the normal path surface the failure.
        Err(_) => return None,
    };
    let confirmed = match two_factor_store().get(&record.id).await {
        Ok(Some(state)) => state.is_confirmed(),
        Ok(None) => false,
        Err(_) => return Some(fail_closed(AUTH_UNAVAILABLE)),
    };
    if !confirmed {
        return None;
    }
    // A confirmed account: re-verify the password here because the normal
    // `login_with_provider` path is skipped entirely.
    if !Argon2Verifier::new().verify(&record.password_hash, password) {
        return Some(json_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "AuthError::BadCredentials",
            INVALID_CREDENTIALS,
        ));
    }
    let pending = SessionUser {
        id: record.id,
        email: Some(record.email),
        email_verified_at: record.email_verified_at,
        timezone: record.timezone,
    };
    match start_challenge(guard, &pending).await {
        Ok(response) => Some(response),
        Err(_) => Some(fail_closed(AUTH_UNAVAILABLE)),
    }
}

/// Persist a pending identity and build the `302` challenge redirect.
async fn start_challenge(guard: &SessionGuard, user: &SessionUser) -> Result<Response, AuthError> {
    let session = Session::new(None, guard.session_store(), None);
    let key = pending_key(guard);
    session
        .insert(&key, user)
        .await
        .map_err(|_| AuthError::StoreUnavailable)?;
    session
        .save()
        .await
        .map_err(|_| AuthError::StoreUnavailable)?;
    let id = session.id().ok_or(AuthError::StoreUnavailable)?;
    let cookie = guard.cookie().build_cookie(id.to_string());
    let mut response = found(CHALLENGE_PATH);
    if let Ok(value) = HeaderValue::try_from(cookie.to_string()) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    Ok(response)
}

/// The session key under which the pending (pre-challenge) identity is stored.
fn pending_key(guard: &SessionGuard) -> String {
    format!("{}two_factor_pending", guard.policy().prefix)
}

/// POST /user/two-factor-authentication — enable, returning the secret + URI.
///
/// Gated by `password.confirm`. Requires an authenticated principal (else `302`
/// → `/login`) and a configured cipher (else `500`, fail closed). On success the
/// plaintext secret, `otpauth://` URI, and recovery codes are returned once;
/// only the sealed secret and hashed codes are persisted.
async fn enable(user: Option<Extension<AuthUser>>) -> Response {
    let Some(Extension(user)) = user else {
        return found(LOGIN_PATH);
    };
    let Some(service) = two_factor_service() else {
        return fail_closed(AUTH_UNAVAILABLE);
    };
    let account = user.email.clone().unwrap_or_else(|| user.id.clone());
    match service.enable(&user.id, &account).await {
        Ok(enrollment) => {
            store_recovery_codes(&user.id, enrollment.recovery_codes.clone());
            json_ok(EnrollmentResponse {
                secret: enrollment.secret,
                otpauth_uri: enrollment.otpauth_uri,
                recovery_codes: enrollment.recovery_codes,
            })
        }
        Err(_) => fail_closed(AUTH_UNAVAILABLE),
    }
}

/// POST /user/confirmed-two-factor-authentication — confirm with a code.
///
/// Gated by `password.confirm`. Verifies the submitted `code` against the
/// pending secret and, on success, stamps `two_factor_confirmed_at`. An invalid
/// code is `422`; any other failure is `500`.
async fn confirm(user: Option<Extension<AuthUser>>, body: axum::body::Bytes) -> Response {
    let Some(Extension(user)) = user else {
        return found(LOGIN_PATH);
    };
    let Some(service) = two_factor_service() else {
        return fail_closed(AUTH_UNAVAILABLE);
    };
    let fields = parse_form(&body);
    let code = field(&fields, "code").unwrap_or_default();
    match service.confirm(&user.id, code).await {
        Ok(()) => json_ok(ConfirmedResponse { confirmed: true }),
        Err(AuthError::InvalidTwoFactorCode) => invalid_code(),
        Err(_) => fail_closed(AUTH_UNAVAILABLE),
    }
}

/// DELETE /user/two-factor-authentication — disable and wipe the secrets.
///
/// Gated by `password.confirm`. Deletes the stored record (secret + recovery
/// hashes) and drops the cached plaintext codes. Disabling is idempotent.
async fn disable(user: Option<Extension<AuthUser>>) -> Response {
    let Some(Extension(user)) = user else {
        return found(LOGIN_PATH);
    };
    let Some(service) = two_factor_service() else {
        return fail_closed(AUTH_UNAVAILABLE);
    };
    match service.disable(&user.id).await {
        Ok(()) => {
            clear_recovery_codes(&user.id);
            json_ok(ConfirmedResponse { confirmed: false })
        }
        Err(_) => fail_closed(AUTH_UNAVAILABLE),
    }
}

/// GET /user/two-factor-recovery-codes — view the current recovery codes.
///
/// Gated by `password.confirm`. The plaintext codes are only available from the
/// process-wide display cache (populated on enable/regenerate); once the cache
/// is gone they cannot be recovered from the stored hashes, so the view answers
/// `404` and the user regenerates.
async fn recovery_codes_show(user: Option<Extension<AuthUser>>) -> Response {
    let Some(Extension(user)) = user else {
        return found(LOGIN_PATH);
    };
    match cached_recovery_codes(&user.id) {
        Some(recovery_codes) => json_ok(RecoveryCodesResponse { recovery_codes }),
        None => json_error(
            StatusCode::NOT_FOUND,
            "AuthError::TwoFactorRecoveryCodesUnavailable",
            RECOVERY_CODES_UNAVAILABLE,
        ),
    }
}

/// POST /user/two-factor-recovery-codes — regenerate a fresh set.
///
/// Gated by `password.confirm`. Requires an existing secret (else `409`); the
/// new plaintext codes are cached for the GET view and returned once.
async fn recovery_codes_regenerate(user: Option<Extension<AuthUser>>) -> Response {
    let Some(Extension(user)) = user else {
        return found(LOGIN_PATH);
    };
    let Some(service) = two_factor_service() else {
        return fail_closed(AUTH_UNAVAILABLE);
    };
    match service.regenerate_recovery_codes(&user.id).await {
        Ok(recovery_codes) => {
            store_recovery_codes(&user.id, recovery_codes.clone());
            json_ok(RecoveryCodesResponse { recovery_codes })
        }
        Err(AuthError::TwoFactorRequired) => json_error(
            StatusCode::CONFLICT,
            "AuthError::TwoFactorRequired",
            TWO_FACTOR_NOT_ENABLED,
        ),
        Err(_) => fail_closed(AUTH_UNAVAILABLE),
    }
}

/// GET /two-factor-challenge — render the challenge form.
async fn challenge_page() -> Result<ViewResponse, ViewError> {
    /// View context for the challenge template (the form is static).
    #[derive(Serialize)]
    struct ChallengeContext {
        /// Form field carrying the six-digit TOTP code.
        code_field: &'static str,
        /// Form field carrying a recovery code.
        recovery_field: &'static str,
    }
    engine().render(
        "auth/two-factor-challenge.html",
        &ChallengeContext {
            code_field: "code",
            recovery_field: "recovery_code",
        },
    )
}

/// POST /two-factor-challenge — verify a code or recovery code, complete login.
///
/// Flow: resolve the guard and the pending session marker (absent → `302` →
/// `/login`); throttle via the `two-factor` limiter keyed on the pending session
/// id (`429` when denied); verify the submitted TOTP `code` or `recovery_code`;
/// on success promote the pending identity into a real session (fresh id) and
/// `303` → home. An invalid code is `422`; every other failure is `500`.
async fn challenge_submit(
    Extension(state): Extension<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(guard) = state.auth::<SessionGuard>() else {
        return fail_closed(AUTH_UNAVAILABLE);
    };
    let Some(session_id) = session_id_from_headers(&headers) else {
        return found(LOGIN_PATH);
    };
    let Ok(id) = session_id.parse::<Id>() else {
        return found(LOGIN_PATH);
    };
    let session = Session::new(Some(id), guard.session_store(), None);
    let key = pending_key(&guard);
    let pending: Option<SessionUser> = match session.get(&key).await {
        Ok(pending) => pending,
        Err(_) => return fail_closed(AUTH_UNAVAILABLE),
    };
    let Some(pending) = pending else {
        return found(LOGIN_PATH);
    };

    let input = LimiterInput::new().with_session_id(&session_id);
    match two_factor_registry().check(TWO_FACTOR, &input) {
        Ok(ThrottleDecision::Allowed { .. }) => {}
        Ok(ThrottleDecision::Denied { retry_after_secs }) => {
            return throttle_response(retry_after_secs);
        }
        Err(_) => return fail_closed(AUTH_UNAVAILABLE),
    }

    let Some(service) = two_factor_service() else {
        return fail_closed(AUTH_UNAVAILABLE);
    };
    let fields = parse_form(&body);
    let code = field(&fields, "code").unwrap_or_default().trim();
    let recovery = field(&fields, "recovery_code").unwrap_or_default().trim();
    let outcome = if recovery.is_empty() {
        service.verify_challenge(&pending.id, code).await
    } else {
        service.consume_recovery_code(&pending.id, recovery).await
    };
    match outcome {
        Ok(()) => complete_login(&guard, session, &key, &pending).await,
        Err(AuthError::InvalidTwoFactorCode) => invalid_code(),
        Err(_) => fail_closed(AUTH_UNAVAILABLE),
    }
}

/// Promote the pending identity into a real session and `303` → home.
///
/// Inserts the identity under the guard's user key, removes the pending marker,
/// cycles the session id (session-fixation defense), saves, and sets the
/// hardened session cookie to the new id.
async fn complete_login(
    guard: &SessionGuard,
    session: Session,
    pending_key: &str,
    pending: &SessionUser,
) -> Response {
    let user_key = guard.policy().user_key();
    if session.insert(&user_key, pending).await.is_err() {
        return fail_closed(AUTH_UNAVAILABLE);
    }
    if session.remove::<SessionUser>(pending_key).await.is_err() {
        return fail_closed(AUTH_UNAVAILABLE);
    }
    if session.cycle_id().await.is_err() {
        return fail_closed(AUTH_UNAVAILABLE);
    }
    if session.save().await.is_err() {
        return fail_closed(AUTH_UNAVAILABLE);
    }
    let Some(new_id) = session.id() else {
        return fail_closed(AUTH_UNAVAILABLE);
    };
    let cookie = guard.cookie().build_cookie(new_id.to_string());
    let mut response = see_other(&fortify_config().home);
    if let Ok(value) = HeaderValue::try_from(cookie.to_string()) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

/// Shared runtime template engine rooted at the views directory.
///
/// Mirrors the web layer's engine selection so `cargo test -p rustasea-app`
/// (crate-root cwd) renders the same on-disk templates as a deployed app.
fn engine() -> &'static MinijinjaEngine {
    static ENGINE: OnceLock<MinijinjaEngine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        if std::path::Path::new(rustasea::view::VIEWS_DIR).is_dir() {
            MinijinjaEngine::from_default_root()
        } else {
            MinijinjaEngine::new(crate::routes::resources_root().join("views"))
        }
    })
}

/// `200 OK` JSON response for the management endpoints.
fn json_ok<T: Serialize>(body: T) -> Response {
    (StatusCode::OK, axum::Json(body)).into_response()
}

/// `422` carrying the typed invalid-code error.
fn invalid_code() -> Response {
    json_error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "AuthError::InvalidTwoFactorCode",
        INVALID_CODE,
    )
}

/// Build the `302 Found` redirect to `location`.
fn found(location: &str) -> Response {
    let value = HeaderValue::try_from(location).unwrap_or_else(|_| HeaderValue::from_static("/"));
    (StatusCode::FOUND, [(header::LOCATION, value)]).into_response()
}
