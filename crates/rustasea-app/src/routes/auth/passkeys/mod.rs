//! Passkey / WebAuthn routes (AUTH-017) — management, login, well-known.
//!
//! Three surfaces:
//!
//! * **Management** (behind the `password.confirm` gate, Fortify's
//!   `confirmPassword` parity): `GET /user/passkeys` lists the caller's
//!   credentials, `POST /user/passkeys` drives the registration ceremony
//!   (begin → creation options; finish → stored credential), and
//!   `DELETE /user/passkeys/{id}` removes one.
//! * **Login**: `POST /passkeys/login` drives the assertion ceremony (begin →
//!   request options + a session cookie binding the challenge; finish →
//!   authenticated session). Throttled by the `passkeys` limiter keyed on the
//!   presented credential id.
//! * **Discovery**: `GET /.well-known/passkey-endpoints` advertises the
//!   enrollment/manage route (the kit's `route("security.edit")` analogue).
//!
//! # Feature gate
//!
//! Every route is gated by `[fortify.features.passkeys].enabled` (off → `404`);
//! the kit removes the routes entirely, a static table cannot, so `404` is the
//! closest honest analogue.
//!
//! # Secrets
//!
//! Public keys are never echoed: the JSON views expose only `id`, `name`, and
//! `created_at`. Challenges are single-use and tracked through the
//! process-wide [`ChallengeStore`](rustasea::auth::ChallengeStore) seam.
//!
//! # Fail closed
//!
//! A missing service, an unknown credential, or a store error denies the
//! ceremony (`500`/`422`) rather than degrading to a permissive path.

pub(crate) mod wiring;

use std::sync::Arc;

#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::{Extension, Path};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use tower_sessions::session::{Id, Session};

use rustasea::auth::{
    AuthError, AuthUser, AuthenticationResponse, LimiterInput, PasskeyCredential,
    RegistrationResponse, SessionGuard, SessionUser, ThrottleDecision, PASSKEYS,
};
use rustasea::http::AppState;
use rustasea::router::Router as RouteTable;
use rustasea::validation::serde_json::{self, Value};

use crate::routes::helpers::{fortify_config, json_error, see_other, user_provider};
use crate::routes::{session_id_from_headers, LOGIN_PATH, PASSWORD_CONFIRM};

use super::{fail_closed, throttle_response, AUTH_UNAVAILABLE};
use wiring::{passkey_service, passkeys_registry};

/// The settings route the kit exposes as `security.edit` (enroll/manage target).
const SECURITY_PATH: &str = "/settings/security";

/// Detail for the `404` returned when `[fortify].features.passkeys` is off.
const PASSKEYS_DISABLED: &str = "Passkeys are disabled.";

/// Detail for a `422` when the JSON ceremony body cannot be parsed.
const INVALID_BODY: &str = "The passkey request body was invalid.";

/// Detail for a `422` when a registration attestation does not verify.
const INVALID_REGISTRATION: &str = "The passkey registration could not be verified.";

/// Detail for a `422` when an assertion does not verify.
const INVALID_ASSERTION: &str = "The passkey assertion could not be verified.";

/// Register the passkey route table onto `table`.
///
/// The management routes live inside a [`RouteTable::group`] because
/// [`RouteTable::middleware`] is sticky — the group scopes the
/// `password.confirm` gate so it cannot leak onto later routes. The login and
/// discovery routes stay ungated (a guest starts the assertion ceremony).
pub(crate) fn register(table: &mut RouteTable) {
    table.get_action("/.well-known/passkey-endpoints", well_known);
    table.post_action("/passkeys/login", login);

    table.group(|group| {
        group
            .middleware(PASSWORD_CONFIRM)
            .get_action("/user/passkeys", list)
            .post_action("/user/passkeys", manage)
            .delete_action("/user/passkeys/{id}", delete);
    });
}

/// JSON view of a credential — never exposes the public key or counter.
#[derive(Serialize)]
struct CredentialView {
    /// Base64url credential id.
    id: String,
    /// User-chosen credential label.
    name: String,
    /// RFC 3339 creation timestamp.
    created_at: String,
}

impl From<PasskeyCredential> for CredentialView {
    /// Project a stored credential onto its safe JSON view.
    fn from(credential: PasskeyCredential) -> Self {
        Self {
            id: credential.id,
            name: credential.name,
            created_at: credential.created_at,
        }
    }
}

/// JSON body for the well-known discovery endpoint.
#[derive(Serialize)]
struct WellKnownResponse {
    /// Route that starts passkey enrollment.
    enroll: &'static str,
    /// Route that manages existing passkeys.
    manage: &'static str,
}

/// JSON body for a successful deletion.
#[derive(Serialize)]
struct DeletedResponse {
    /// Always `true` on success.
    deleted: bool,
}

/// GET /.well-known/passkey-endpoints — advertise enroll/manage routes.
async fn well_known() -> Response {
    if passkeys_off() {
        return not_found();
    }
    json_ok(WellKnownResponse {
        enroll: SECURITY_PATH,
        manage: SECURITY_PATH,
    })
}

/// GET /user/passkeys — list the authenticated user's credentials.
///
/// Gated by `password.confirm`. Requires an authenticated principal (else `302`
/// → `/login`). Returns a JSON array of safe credential views.
async fn list(user: Option<Extension<AuthUser>>) -> Response {
    if passkeys_off() {
        return not_found();
    }
    let Some(Extension(user)) = user else {
        return found(LOGIN_PATH);
    };
    let Some(service) = passkey_service() else {
        return fail_closed(AUTH_UNAVAILABLE);
    };
    match service.list_credentials(&user.id).await {
        Ok(credentials) => {
            let views: Vec<CredentialView> =
                credentials.into_iter().map(CredentialView::from).collect();
            json_ok(views)
        }
        Err(_) => fail_closed(AUTH_UNAVAILABLE),
    }
}

/// POST /user/passkeys — begin or finish a registration ceremony.
///
/// Gated by `password.confirm`. A body **without** a `response` key begins the
/// ceremony and returns the `PublicKeyCredentialCreationOptions`; a body
/// **with** a `response` key completes it, verifies the attestation, and stores
/// the credential bound to the caller. An invalid attestation is `422`; any
/// other failure is `500`.
async fn manage(user: Option<Extension<AuthUser>>, body: axum::body::Bytes) -> Response {
    if passkeys_off() {
        return not_found();
    }
    let Some(Extension(user)) = user else {
        return found(LOGIN_PATH);
    };
    let Some(service) = passkey_service() else {
        return fail_closed(AUTH_UNAVAILABLE);
    };
    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return invalid_body();
    };
    let key = registration_key(&user.id);
    if payload.get("response").is_some() {
        let Ok(response) = serde_json::from_value::<RegistrationResponse>(payload) else {
            return invalid_body();
        };
        let name = response
            .name
            .clone()
            .unwrap_or_else(|| "Passkey".to_string());
        match service
            .finish_registration(&user.id, &key, &response, &name)
            .await
        {
            Ok(credential) => json_ok(CredentialView::from(credential)),
            Err(AuthError::Passkey(_)) => invalid_registration(),
            Err(_) => fail_closed(AUTH_UNAVAILABLE),
        }
    } else {
        let account = user.email.clone().unwrap_or_else(|| user.id.clone());
        match service
            .begin_registration(&user.id, &account, &account, &key)
            .await
        {
            Ok(options) => json_ok(options),
            Err(_) => fail_closed(AUTH_UNAVAILABLE),
        }
    }
}

/// DELETE /user/passkeys/{id} — remove one of the caller's credentials.
///
/// Gated by `password.confirm`. Deletion is idempotent and ownership-scoped: a
/// credential belonging to another user is never touched.
async fn delete(user: Option<Extension<AuthUser>>, Path(id): Path<String>) -> Response {
    if passkeys_off() {
        return not_found();
    }
    let Some(Extension(user)) = user else {
        return found(LOGIN_PATH);
    };
    let Some(service) = passkey_service() else {
        return fail_closed(AUTH_UNAVAILABLE);
    };
    match service.delete_credential(&user.id, &id).await {
        Ok(()) => json_ok(DeletedResponse { deleted: true }),
        Err(_) => fail_closed(AUTH_UNAVAILABLE),
    }
}

/// POST /passkeys/login — begin or finish an assertion ceremony.
///
/// Ungated. A body **without** a `response` key begins the ceremony: a fresh
/// empty session is minted, its id keys the stored challenge, and the request
/// options plus the session cookie are returned. A body **with** a `response`
/// key completes it: the challenge is consumed, the assertion is verified under
/// the `passkeys` limiter (keyed on the credential id), and the owning user is
/// promoted into a real session (`303` → home). An invalid assertion is `422`;
/// a denied throttle is `429`.
async fn login(
    Extension(state): Extension<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if passkeys_off() {
        return not_found();
    }
    let Some(guard) = state.auth::<SessionGuard>() else {
        return fail_closed(AUTH_UNAVAILABLE);
    };
    let Some(service) = passkey_service() else {
        return fail_closed(AUTH_UNAVAILABLE);
    };
    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return invalid_body();
    };
    if payload.get("response").is_some() {
        login_finish(&guard, &service, &headers, payload).await
    } else {
        login_begin(&guard, &service).await
    }
}

/// Begin an assertion ceremony: mint a session, store the challenge, reply.
async fn login_begin(guard: &SessionGuard, service: &rustasea::auth::PasskeyService) -> Response {
    let session = Session::new(None, guard.session_store(), None);
    if session.save().await.is_err() {
        return fail_closed(AUTH_UNAVAILABLE);
    }
    let Some(id) = session.id() else {
        return fail_closed(AUTH_UNAVAILABLE);
    };
    let key = login_key(&id.to_string());
    match service.begin_authentication(&key, &[]).await {
        Ok(options) => {
            let cookie = guard.cookie().build_cookie(id.to_string());
            let mut response = json_ok(options);
            if let Ok(value) = HeaderValue::try_from(cookie.to_string()) {
                response.headers_mut().append(header::SET_COOKIE, value);
            }
            response
        }
        Err(_) => fail_closed(AUTH_UNAVAILABLE),
    }
}

/// Finish an assertion ceremony: throttle, verify, promote the session.
async fn login_finish(
    guard: &SessionGuard,
    service: &rustasea::auth::PasskeyService,
    headers: &HeaderMap,
    payload: Value,
) -> Response {
    let Ok(assertion) = serde_json::from_value::<AuthenticationResponse>(payload) else {
        return invalid_body();
    };
    let input = LimiterInput::new().with_credential_id(&assertion.id);
    match passkeys_registry().check(PASSKEYS, &input) {
        Ok(ThrottleDecision::Allowed { .. }) => {}
        Ok(ThrottleDecision::Denied { retry_after_secs }) => {
            return throttle_response(retry_after_secs);
        }
        Err(_) => return fail_closed(AUTH_UNAVAILABLE),
    }
    let Some(session_id) = session_id_from_headers(headers) else {
        return found(LOGIN_PATH);
    };
    let key = login_key(&session_id);
    match service.finish_authentication(&key, &assertion).await {
        Ok(credential) => complete_login(guard, &session_id, &credential.user_id).await,
        Err(
            AuthError::InvalidPasskeyAssertion | AuthError::PasskeyRequired | AuthError::Passkey(_),
        ) => invalid_assertion(),
        Err(_) => fail_closed(AUTH_UNAVAILABLE),
    }
}

/// Promote the credential owner into a real session and `303` → home.
///
/// Resolves the user record, inserts a [`SessionUser`] under the guard's user
/// key, cycles the session id (session-fixation defense), saves, and sets the
/// hardened session cookie to the new id.
async fn complete_login(guard: &SessionGuard, session_id: &str, user_id: &str) -> Response {
    let record = match user_provider().find_by_id(user_id).await {
        Ok(Some(record)) => record,
        // A credential bound to a vanished user must not authenticate.
        Ok(None) => return invalid_assertion(),
        Err(_) => return fail_closed(AUTH_UNAVAILABLE),
    };
    let Ok(id) = session_id.parse::<Id>() else {
        return found(LOGIN_PATH);
    };
    let session = Session::new(Some(id), guard.session_store(), None);
    let user = SessionUser {
        id: record.id,
        email: Some(record.email),
        email_verified_at: record.email_verified_at,
    };
    let user_key = guard.policy().user_key();
    if session.insert(&user_key, &user).await.is_err() {
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

/// The challenge-store key for a user's pending registration.
fn registration_key(user_id: &str) -> String {
    format!("passkey:register:{user_id}")
}

/// The challenge-store key for a pending assertion bound to `session_id`.
fn login_key(session_id: &str) -> String {
    format!("passkey:login:{session_id}")
}

/// Whether passkeys are disabled (test override included).
fn passkeys_off() -> bool {
    #[cfg(test)]
    if PASSKEYS_OFF.load(Ordering::SeqCst) {
        return true;
    }
    !fortify_config().features.passkeys.enabled
}

/// Test-only passkey-disable flag; see [`set_passkeys_disabled`].
#[cfg(test)]
static PASSKEYS_OFF: AtomicBool = AtomicBool::new(false);

/// Force the passkeys feature gate off (tests serialize via the lock).
#[cfg(test)]
pub(crate) fn set_passkeys_disabled(off: bool) {
    PASSKEYS_OFF.store(off, Ordering::SeqCst);
}

/// `404` returned when the passkeys feature is disabled.
fn not_found() -> Response {
    json_error(StatusCode::NOT_FOUND, "PasskeysDisabled", PASSKEYS_DISABLED)
}

/// `200 OK` JSON response for the passkey endpoints.
fn json_ok<T: Serialize>(body: T) -> Response {
    (StatusCode::OK, axum::Json(body)).into_response()
}

/// `422` for an unparseable ceremony body.
fn invalid_body() -> Response {
    json_error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "ValidationError",
        INVALID_BODY,
    )
}

/// `422` carrying the typed invalid-registration error.
fn invalid_registration() -> Response {
    json_error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "AuthError::Passkey",
        INVALID_REGISTRATION,
    )
}

/// `422` carrying the typed invalid-assertion error.
fn invalid_assertion() -> Response {
    json_error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "AuthError::InvalidPasskeyAssertion",
        INVALID_ASSERTION,
    )
}

/// Build the `302 Found` redirect to `location`.
fn found(location: &str) -> Response {
    let value = HeaderValue::try_from(location).unwrap_or_else(|_| HeaderValue::from_static("/"));
    (StatusCode::FOUND, [(header::LOCATION, value)]).into_response()
}
