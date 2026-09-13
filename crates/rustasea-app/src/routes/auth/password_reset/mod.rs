//! Password-reset flow (AUTH-013) — request a link, render the form, consume it.
//!
//! Split out of [`super`] (held at the 500-line cap). Four handlers, mirroring
//! the kit/Fortify route names:
//!
//! * `GET /forgot-password` (`password.request`) — render the request form.
//! * `POST /forgot-password` (`password.email`) — mint a token, store its
//!   **hash**, sign a reset link, mail it, `303` → `/forgot-password`.
//! * `GET /reset-password/{token}` (`password.reset`) — verify the signed link
//!   and the stored token, render the reset form.
//! * `POST /reset-password` (`password.store`) — verify the signed link + stored
//!   token, validate the new password, write the new hash, delete the token.
//!
//! # Account-enumeration resistance
//!
//! The request route answers the **same** `303` → `/forgot-password` whether or
//! not the email exists, so a caller cannot tell accounts apart. It is the same
//! principle as `login_submit`'s byte-identical `422`.
//!
//! # Hashed token, single-use (kit parity)
//!
//! The plaintext token is mailed; only an Argon2 hash is persisted (see
//! [`rustasea::auth::PasswordResetStore`]). On a successful reset the token row
//! is deleted, so a link can be used once. The link is *also* an HMAC-signed,
//! expiring URL ([`SignedUrlSigner`]) whose TTL is the broker's `expire`
//! (minutes → seconds).
//!
//! # Fail-closed
//!
//! A missing signing key, an unresolvable mailer, or a store failure is a `500`
//! — never a silent success. A tampered, malformed, or expired link is a typed
//! `403`. With `[fortify].features.reset_passwords` off every route is `404`
//! (the kit removes them; a static table cannot, so `404` is the honest analogue).
//!
//! # Session invalidation gap (documented, not faked)
//!
//! Laravel logs the user out of all devices after a reset. RustaSea's
//! [`SessionGuard`](rustasea::auth::SessionGuard) only destroys a session *by
//! its own id* ([`Guard::logout`](rustasea::auth::Guard::logout)); there is no
//! "invalidate every session for user X" primitive, so this handler cannot do
//! it. The gap is documented rather than simulated.

mod wiring;

use axum::body::Bytes;
use axum::extract::{Path, RawQuery};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

use rustasea::auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea::auth::{
    generate_token, LimiterInput, SignedUrlError, SignedUrlSigner, ThrottleDecision,
};
use rustasea::validation::serde_json::{Map, Value};
use rustasea::validation::{PasswordPolicy, Rules};
use rustasea_mail::{Mail, MailAddress, Mailable};

use crate::routes::helpers::{field, json_error, parse_form, see_other, user_provider};
use crate::routes::LOGIN_PATH;

use super::{fail_closed, throttle_response, AUTH_UNAVAILABLE};
use wiring::{
    app_url, broker_expire_minutes, mailer, reset_off, reset_registry, reset_store, signer,
    MAILER_UNAVAILABLE, PASSWORD_EMAIL, SIGNER_UNAVAILABLE, STORE_UNAVAILABLE,
};

#[cfg(test)]
pub(crate) use wiring::{install_reset_store, install_signer, set_reset_passwords_disabled};

/// The request-form page (`GET /forgot-password`).
const FORGOT_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Reset your password</title></head>
<body><main><h1>Reset your password</h1>
<p>Enter your email address and we will send you a reset link.</p>
<form method="post" action="/forgot-password">
<label>Email <input type="email" name="email" required></label>
<button type="submit">Email reset link</button>
</form>
</main></body></html>"#;

/// Redirect target after a request (the neutral, enumeration-safe response).
const FORGOT_PATH: &str = "/forgot-password";

/// The `password.store` POST target (the reset form's `action`).
const RESET_PATH: &str = "/reset-password";

/// `404` detail when `[fortify].features.reset_passwords` is off.
const RESET_DISABLED: &str = "Password resets are disabled.";
/// `422` detail when the request form omits a usable email.
const EMAIL_REQUIRED: &str = "The email field is required.";
/// `422` detail when the new password fails validation.
const PASSWORD_INVALID: &str = "The password field is invalid.";
/// `403` detail for an invalid, tampered, or expired link.
const INVALID_LINK: &str = "This password reset link is invalid or has expired.";

/// GET /forgot-password — render the request form (feature-gated → `404`).
pub(super) async fn forgot_password_page() -> Response {
    if reset_off() {
        return reset_disabled();
    }
    Html(FORGOT_HTML).into_response()
}

/// POST /forgot-password — mint, store the hash, sign, mail, `303` neutrally.
///
/// Feature-gated (`404`). Throttled by [`PASSWORD_EMAIL`] (`429`). The response
/// is the same `303` → [`FORGOT_PATH`] for a known and an unknown email. A
/// missing email is a `422`; a provider, signer, store, or mailer failure is a
/// `500` (fail closed).
pub(super) async fn forgot_password_submit(body: Bytes) -> Response {
    if reset_off() {
        return reset_disabled();
    }
    let fields = parse_form(&body);
    let email = match field(&fields, "email") {
        Some(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => {
            return json_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "ValidationError",
                EMAIL_REQUIRED,
            )
        }
    };

    // Throttle before any credential work, so known/unknown emails behave the
    // same. A missing limiter is a wiring bug: fail closed (500).
    let input = LimiterInput::new().with_username(&email);
    match reset_registry().check(PASSWORD_EMAIL, &input) {
        Ok(ThrottleDecision::Allowed { .. }) => {}
        Ok(ThrottleDecision::Denied { retry_after_secs }) => {
            return throttle_response(retry_after_secs);
        }
        Err(_) => return fail_closed(AUTH_UNAVAILABLE),
    }

    // Resolve the account. A provider error fails closed (500); an unknown
    // email takes the same neutral path as a known one (no mail is sent).
    let provider = user_provider();
    let record = match provider.find_by_email(&email).await {
        Ok(found) => found,
        Err(_) => return fail_closed(AUTH_UNAVAILABLE),
    };
    let Some(user) = record else {
        return see_other(FORGOT_PATH);
    };

    let Some(signer) = signer() else {
        return fail_closed(SIGNER_UNAVAILABLE);
    };
    // Store only the HASH of the token (kit parity: Laravel hashes it too).
    let token = generate_token();
    let Ok(token_hash) = Argon2Verifier::new().hash(&token) else {
        return fail_closed(STORE_UNAVAILABLE);
    };
    let now = chrono::Utc::now().timestamp();
    if reset_store()
        .create(&user.email, &token_hash, now)
        .await
        .is_err()
    {
        return fail_closed(STORE_UNAVAILABLE);
    }

    let Ok(link) = reset_link(&signer, &user.email, &token) else {
        return fail_closed(SIGNER_UNAVAILABLE);
    };
    if mailer().is_none() {
        return fail_closed(MAILER_UNAVAILABLE);
    }
    let mail = ResetPasswordMail {
        to: user.email.clone(),
        link,
    };
    if Mail::send(&mail).await.is_err() {
        return fail_closed(MAILER_UNAVAILABLE);
    }
    see_other(FORGOT_PATH)
}

/// GET /reset-password/{token} — verify the link and render the reset form.
///
/// Verifies the HMAC + expiry ([`SignedUrlSigner::verify_now`]) and that a
/// matching, unexpired stored token exists. Any failure is a `403` (a missing
/// key is a `500` misconfiguration). The `email`, `token`, `expires`, and
/// `signature` are echoed as hidden fields so the POST can re-verify the link.
pub(super) async fn reset_password_page(
    Path(token): Path<String>,
    RawQuery(query): RawQuery,
) -> Response {
    if reset_off() {
        return reset_disabled();
    }
    let query = query.unwrap_or_default();
    let path = format!("/reset-password/{token}");
    let Some(signer) = signer() else {
        return fail_closed(SIGNER_UNAVAILABLE);
    };
    if let Err(error) = signer.verify_now(&path, &query) {
        return match error {
            SignedUrlError::MissingKey => fail_closed(SIGNER_UNAVAILABLE),
            other => forbidden(other.code()),
        };
    }
    let fields = parse_form(query.as_bytes());
    let Some(email) = field(&fields, "email") else {
        return forbidden("MissingSignature");
    };
    if !token_matches(email, &token).await {
        return forbidden("InvalidSignature");
    }
    let expires = field(&fields, "expires").unwrap_or_default();
    let signature = field(&fields, "signature").unwrap_or_default();
    Html(render_reset_form(email, &token, expires, signature)).into_response()
}

/// POST /reset-password — verify, validate, consume the token, write the new hash.
///
/// Verifies the signed link (rebuilt from the hidden fields), the stored token
/// hash, and its expiry; then validates the new password
/// (`required|string|password|confirmed` + [`PasswordPolicy::production`]).
/// A validation failure is a `422`; an invalid/expired/reused link is a `403`.
///
/// # Delete-then-write (fail-closed single-use)
///
/// The token is consumed (deleted) **before** the new hash is written, not
/// after. The token is the single-use secret; if the delete ran last and it
/// failed — or the process died between the two steps — a still-valid token
/// could be replayed, breaking the single-use guarantee. Deleting first makes
/// a failure between the steps safe: the token is gone, so it cannot be reused,
/// and the user simply requests a fresh link. A consumed-but-unused token is
/// safe; a reusable token is not. This is the deliberate fail-closed direction.
///
/// On success the hash is written with [`UserProvider::update_password`] — the
/// `forceFill` analogue: the provider writes the hash column directly, bypassing
/// any fillable guard — and the response is `303` → `/login`.
///
/// [`UserProvider::update_password`]: rustasea::auth::UserProvider::update_password
pub(super) async fn reset_password_submit(body: Bytes) -> Response {
    if reset_off() {
        return reset_disabled();
    }
    let fields = parse_form(&body);
    let email = field(&fields, "email")
        .unwrap_or_default()
        .trim()
        .to_string();
    let token = field(&fields, "token").unwrap_or_default().to_string();
    let expires = field(&fields, "expires").unwrap_or_default().to_string();
    let signature = field(&fields, "signature").unwrap_or_default().to_string();
    if email.is_empty() || token.is_empty() {
        return forbidden("MissingSignature");
    }

    let Some(signer) = signer() else {
        return fail_closed(SIGNER_UNAVAILABLE);
    };
    let path = format!("/reset-password/{token}");
    let query = signed_query(&email, &expires, &signature);
    if let Err(error) = signer.verify_now(&path, &query) {
        return match error {
            SignedUrlError::MissingKey => fail_closed(SIGNER_UNAVAILABLE),
            other => forbidden(other.code()),
        };
    }

    // The stored hash is the authoritative single-use secret; a missing,
    // expired, or mismatched token is an invalid link.
    if !token_matches(&email, &token).await {
        return forbidden("InvalidSignature");
    }

    // A missing field is an absent key (fails `required`), not `""`.
    let mut payload = Map::new();
    for key in ["password", "password_confirmation"] {
        if let Some(value) = field(&fields, key) {
            payload.insert(key.to_string(), Value::String(value.to_string()));
        }
    }
    let rules = Rules::new()
        .password_policy(PasswordPolicy::production())
        .field("password", "required|string|password|confirmed");
    if rules.validate(&Value::Object(payload)).is_err() {
        return json_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "ValidationError",
            PASSWORD_INVALID,
        );
    }
    let password = field(&fields, "password").unwrap_or_default().to_string();

    let provider = user_provider();
    let user = match provider.find_by_email(&email).await {
        Ok(Some(user)) => user,
        Ok(None) => return forbidden("InvalidSignature"),
        Err(_) => return fail_closed(AUTH_UNAVAILABLE),
    };
    let Ok(hash) = Argon2Verifier::new().hash(&password) else {
        return fail_closed(STORE_UNAVAILABLE);
    };
    // Single-use: consume (delete) the token BEFORE writing the new hash. If
    // the write then fails, the token is already gone, so it cannot be
    // replayed — the user requests a fresh link (see the handler DocBlock for
    // the delete-then-write rationale).
    if reset_store().delete(&email).await.is_err() {
        return fail_closed(STORE_UNAVAILABLE);
    }
    if provider.update_password(&user.id, &hash).await.is_err() {
        return fail_closed(STORE_UNAVAILABLE);
    }
    // NOTE: existing sessions are NOT invalidated — see the module docs (no
    // per-user invalidation primitive exists on `SessionGuard`).
    see_other(LOGIN_PATH)
}

/// Whether `email` holds an unexpired token whose hash matches `token`.
///
/// Fail-closed: an absent row, an expired row, a store error, or a hash
/// mismatch all return `false`.
async fn token_matches(email: &str, token: &str) -> bool {
    let Ok(Some(record)) = reset_store().find(email).await else {
        return false;
    };
    let expires_at = record.created_at + (broker_expire_minutes() as i64) * 60;
    if chrono::Utc::now().timestamp() > expires_at {
        return false;
    }
    Argon2Verifier::new().verify(&record.token_hash, token)
}

/// Build the absolute signed reset URL for `email`/`token`.
///
/// The signed path is `/reset-password/{token}` with `email` as a signed extra
/// parameter; the TTL is the broker's `expire` in seconds.
fn reset_link(
    signer: &SignedUrlSigner,
    email: &str,
    token: &str,
) -> Result<String, SignedUrlError> {
    let path = format!("/reset-password/{token}");
    let ttl_secs = broker_expire_minutes() * 60;
    let query = signer.sign_expiring(&path, ttl_secs, &[("email", email)])?;
    Ok(format!("{}{path}?{query}", app_url()))
}

/// Rebuild the signed query fragment from the hidden form fields.
///
/// The signature is order-independent and percent-decodes on verify, so a
/// minimal escape of `email` is sufficient; `expires`/`signature` are already
/// url-safe.
fn signed_query(email: &str, expires: &str, signature: &str) -> String {
    format!(
        "email={}&expires={}&signature={}",
        encode_component(email),
        expires,
        signature
    )
}

/// Percent-encode `input`, preserving only the RFC 3986 unreserved set.
fn encode_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &byte in input.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Render the reset form with the signed-link fields as hidden inputs.
fn render_reset_form(email: &str, token: &str, expires: &str, signature: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Choose a new password</title></head>
<body><main><h1>Choose a new password</h1>
<form method="post" action="{RESET_PATH}">
<input type="hidden" name="email" value="{email}">
<input type="hidden" name="token" value="{token}">
<input type="hidden" name="expires" value="{expires}">
<input type="hidden" name="signature" value="{signature}">
<label>New password <input type="password" name="password" required></label>
<label>Confirm password <input type="password" name="password_confirmation" required></label>
<button type="submit">Reset password</button>
</form>
</main></body></html>"#,
        email = html_escape(email),
        token = html_escape(token),
        expires = html_escape(expires),
        signature = html_escape(signature),
    )
}

/// Escape the five HTML metacharacters for safe attribute interpolation.
fn html_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// A mailable carrying the signed reset link.
struct ResetPasswordMail {
    /// Recipient address.
    to: String,
    /// Absolute signed reset URL.
    link: String,
}

impl Mailable for ResetPasswordMail {
    /// Subject line for the reset email.
    fn subject(&self) -> String {
        "Reset your password".to_string()
    }

    /// The single recipient (the account owner).
    fn to(&self) -> Vec<MailAddress> {
        vec![MailAddress::from_email(self.to.clone())]
    }

    /// HTML body containing the signed reset link.
    fn html_body(&self) -> String {
        format!(
            "<p>You requested a password reset. Follow the link below to choose a new password.</p>\
             <p><a href=\"{}\">Reset your password</a></p>",
            self.link
        )
    }
}

/// `404` returned when password resets are disabled.
fn reset_disabled() -> Response {
    json_error(StatusCode::NOT_FOUND, "ResetDisabled", RESET_DISABLED)
}

/// `403` carrying a typed signed-URL error code.
fn forbidden(code: &str) -> Response {
    json_error(StatusCode::FORBIDDEN, code, INVALID_LINK)
}
