//! Email-verification flow (AUTH-014) — send, verify via signed link, resend.
//!
//! Split out of [`super`] (held at the 500-line cap). Three handlers:
//!
//! * `GET /verify-email` — the notice screen (unchanged, now owned here).
//! * `POST /email/verification-notification` — resend: build a signed link,
//!   mail it, throttle to 6/minute (kit parity), `303` → `/verify-email`.
//! * `GET /email/verify/{user_id}/{hash}` — the link target: verify the HMAC +
//!   expiry, check the `hash` against SHA-256 of the user's email, mark the
//!   address verified, `303` → `/dashboard`.
//!
//! # Link shape
//!
//! The signed path is `/email/verify/{user_id}/{hash}` where `hash` is the
//! lowercase-hex SHA-256 of the user's email. The email is *also* carried as a
//! signed `extra` query parameter so the verify handler can resolve the record
//! (the [`UserProvider`] trait has no by-id lookup) and re-check the hash. The
//! link TTL is [`VERIFICATION_TTL_SECS`] (1 hour, matching the kit's default
//! temporary-signed-URL window). A tampered path/query or an expired link is a
//! typed [`SignedUrlError`] mapped to `403`; the signature key is
//! [`AppConfig::key`], so a blank key fails closed (`500`).
//!
//! # Events
//!
//! The kit dispatches a `Verified` event on success. This app has no event
//! dispatcher wired into the request path, so none is fired; marking an
//! already-verified address is idempotent (`set_email_verified_at` rewrites the
//! same value). That omission is deliberate and documented rather than faked.
//!
//! # Skipped route
//!
//! `POST /email/verify/{user_id}/{hash}` is **not** implemented: the signed
//! link is followed by a browser as a `GET`, and there is no POST-only step to
//! mirror. Omitting it is intentional.
//!
//! [`UserProvider`]: rustasea::auth::UserProvider

#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::RwLock;
use std::sync::{Arc, OnceLock};

use axum::extract::{Extension, Path, RawQuery};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};

use rustasea::auth::{
    AuthUser, Limit, LimiterDefinition, LimiterInput, MemoryRateLimiter, RateLimiterRegistry,
    SignedUrlError, SignedUrlSigner, ThrottleDecision,
};
use rustasea::foundation::AppConfig;
use rustasea::ConfigLoader;
use rustasea_mail::{mailer_from_config, Mail, MailAddress, MailConfig, Mailable, Mailer};

use crate::routes::helpers::{
    field, fortify_config, json_error, parse_form, see_other, user_provider,
};
use crate::routes::{session_id_from_headers, LOGIN_PATH};

use super::{fail_closed, throttle_response, AUTH_UNAVAILABLE};

/// Email-verification notice markup (placeholder).
const VERIFY_EMAIL_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Verify your email</title></head>
<body><main><h1>Verify your email</h1>
<p>We sent a verification link to your email address. Follow it to continue.</p>
<form method="post" action="/email/verification-notification">
<button type="submit">Resend verification email</button>
</form>
</main></body></html>"#;

/// Lifetime of a verification link, in seconds (1 hour, kit default).
const VERIFICATION_TTL_SECS: u64 = 3_600;

/// Named limiter for the resend route (kit: `throttle:6,1`).
const VERIFICATION_SEND: &str = "verification.send";

/// Resend limit: 6 attempts per minute, matching the kit's `verification.send`.
const VERIFICATION_MAX_ATTEMPTS: u32 = 6;

/// Detail for the `404` when `[fortify].features.email_verification` is off.
///
/// The kit removes the routes entirely; a static table cannot, so `404` is the
/// closest honest analogue (the same posture AUTH-009 uses for registration).
const VERIFICATION_DISABLED: &str = "Email verification is disabled.";

/// Detail for the `500` when no signing key is configured (fail closed).
const SIGNER_UNAVAILABLE: &str = "Email verification is temporarily unavailable.";

/// Detail for the `500` when no mail transport can be resolved (fail closed).
const MAILER_UNAVAILABLE: &str = "Email delivery is temporarily unavailable.";

/// Detail for the `403` returned for an invalid, tampered, or expired link.
const INVALID_LINK: &str = "This verification link is invalid or has expired.";

/// GET /verify-email — render the verification notice.
pub(super) async fn verify_email_page() -> Html<&'static str> {
    Html(VERIFY_EMAIL_HTML)
}

/// POST /email/verification-notification — send (or resend) the signed link.
///
/// Feature-gated on `[fortify].features.email_verification` (off → `404`).
/// Authenticated-only (no principal → `302` → `/login`). Throttled to
/// [`VERIFICATION_MAX_ATTEMPTS`] per minute by the named `verification.send`
/// limiter, keyed on the session id (falling back to the email). On success the
/// signed link is mailed and the response is `303` → `/verify-email`. A missing
/// signer/mailer, an unresolvable record, or a store failure fails closed
/// (`500`); a throttled request is `429`.
pub(super) async fn verify_email_resend(
    user: Option<Extension<AuthUser>>,
    headers: HeaderMap,
) -> Response {
    if email_verification_off() {
        return verification_disabled();
    }
    let Some(Extension(user)) = user else {
        return found(LOGIN_PATH);
    };
    let Some(email) = user.email.as_deref() else {
        return fail_closed(AUTH_UNAVAILABLE);
    };

    // Throttle first (kit parity: the limiter guards the send action). The key
    // prefers the session id and falls back to the email; a missing identity
    // denies for the full window (the crate's fail-closed default).
    let session_id = session_id_from_headers(&headers).unwrap_or_default();
    let input = LimiterInput::new()
        .with_session_id(&session_id)
        .with_username(email);
    match verification_registry().check(VERIFICATION_SEND, &input) {
        Ok(ThrottleDecision::Allowed { .. }) => {}
        Ok(ThrottleDecision::Denied { retry_after_secs }) => {
            return throttle_response(retry_after_secs);
        }
        Err(_) => return fail_closed(AUTH_UNAVAILABLE),
    }

    // Resolve the stored record: the authenticated user must exist (fail closed).
    let provider = user_provider();
    let Ok(Some(record)) = provider.find_by_email(email).await else {
        return fail_closed(AUTH_UNAVAILABLE);
    };

    let Some(signer) = signer() else {
        return fail_closed(SIGNER_UNAVAILABLE);
    };
    let hash = email_hash(&record.email);
    let path = format!("/email/verify/{}/{}", record.id, hash);
    let Ok(query) = signer.sign_expiring(&path, VERIFICATION_TTL_SECS, &[("email", &record.email)])
    else {
        return fail_closed(SIGNER_UNAVAILABLE);
    };
    let link = format!("{}{path}?{query}", app_url());

    if mailer().is_none() {
        return fail_closed(MAILER_UNAVAILABLE);
    }
    let mail = VerifyEmailMail {
        to: record.email.clone(),
        link,
    };
    if Mail::send(&mail).await.is_err() {
        return fail_closed(MAILER_UNAVAILABLE);
    }
    see_other("/verify-email")
}

/// GET /email/verify/{user_id}/{hash} — consume a signed verification link.
///
/// Feature-gated (off → `404`). Verifies the HMAC and expiry with
/// [`SignedUrlSigner::verify_now`]; a failure other than a missing key is `403`
/// with the typed [`SignedUrlError::code`]. A missing key is `500` (misconfig).
/// The signed `email` parameter is then resolved through the provider, the
/// `{user_id}`/`{hash}` segments are checked against the stored record, and the
/// address is marked verified with the current RFC 3339 timestamp. Any mismatch
/// is `403`; success is `303` → `/dashboard`.
pub(super) async fn verify_email_confirm(
    Path((user_id, hash)): Path<(String, String)>,
    RawQuery(query): RawQuery,
) -> Response {
    if email_verification_off() {
        return verification_disabled();
    }
    let query = query.unwrap_or_default();
    let path = format!("/email/verify/{user_id}/{hash}");
    let Some(signer) = signer() else {
        return fail_closed(SIGNER_UNAVAILABLE);
    };
    if let Err(error) = signer.verify_now(&path, &query) {
        // A missing signing key is a configuration fault (500); every other
        // failure — tampered, malformed, or expired — is a typed 403.
        return match error {
            SignedUrlError::MissingKey => fail_closed(SIGNER_UNAVAILABLE),
            other => forbidden(other.code()),
        };
    }

    // The signed `email` extra param is the identity the link was minted for.
    let fields = parse_form(query.as_bytes());
    let Some(email) = field(&fields, "email") else {
        return forbidden("MissingSignature");
    };
    let provider = user_provider();
    let Ok(Some(record)) = provider.find_by_email(email).await else {
        return forbidden("InvalidSignature");
    };
    // The signature is valid but must also bind to this user + email hash.
    if record.id != user_id || email_hash(&record.email) != hash {
        return forbidden("InvalidSignature");
    }

    let now = chrono::Utc::now().to_rfc3339();
    if provider
        .set_email_verified_at(&record.id, Some(&now))
        .await
        .is_err()
    {
        return fail_closed(AUTH_UNAVAILABLE);
    }
    see_other("/dashboard")
}

/// A mailable carrying the signed verification link.
///
/// Defined here (not in a shared module) because it is the only mail this app
/// sends; the [`Mailable`] contract keeps it transport-agnostic.
struct VerifyEmailMail {
    /// Recipient address.
    to: String,
    /// Absolute signed verification URL.
    link: String,
}

impl Mailable for VerifyEmailMail {
    /// Subject line for the verification email.
    fn subject(&self) -> String {
        "Verify your email address".to_string()
    }

    /// The single recipient (the address being verified).
    fn to(&self) -> Vec<MailAddress> {
        vec![MailAddress::from_email(self.to.clone())]
    }

    /// HTML body containing the signed link.
    fn html_body(&self) -> String {
        format!(
            "<p>Please confirm your email address by following the link below.</p>\
             <p><a href=\"{}\">Verify email address</a></p>",
            self.link
        )
    }
}

/// Lowercase-hex SHA-256 of `email` — the `{hash}` link segment.
pub(crate) fn email_hash(email: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(email.as_bytes());
    hex::encode(hasher.finalize())
}

/// Whether email verification is disabled (test override included).
fn email_verification_off() -> bool {
    #[cfg(test)]
    if EMAIL_VERIFICATION_OFF.load(Ordering::SeqCst) {
        return true;
    }
    !fortify_config().features.email_verification
}

/// Test-only email-verification disable flag.
#[cfg(test)]
static EMAIL_VERIFICATION_OFF: AtomicBool = AtomicBool::new(false);

/// Force the email-verification feature gate off (tests serialize via the lock).
#[cfg(test)]
pub(crate) fn set_email_verification_disabled(off: bool) {
    EMAIL_VERIFICATION_OFF.store(off, Ordering::SeqCst);
}

/// `404` returned when email verification is disabled.
fn verification_disabled() -> Response {
    json_error(
        StatusCode::NOT_FOUND,
        "VerificationDisabled",
        VERIFICATION_DISABLED,
    )
}

/// `403` carrying a typed signed-URL error code.
fn forbidden(code: &str) -> Response {
    json_error(StatusCode::FORBIDDEN, code, INVALID_LINK)
}

/// Build the `302 Found` redirect to `location`.
///
/// Distinct from [`see_other`](crate::routes::helpers::see_other) (`303`): the
/// gate sends an unauthenticated browser to the login screen with a plain
/// `302 Found`.
fn found(location: &'static str) -> Response {
    (
        StatusCode::FOUND,
        [(header::LOCATION, HeaderValue::from_static(location))],
    )
        .into_response()
}

/// The application base URL, used to make the signed link absolute.
///
/// Falls back to `http://localhost` when the config cannot be read, so a
/// misconfigured URL never aborts the send (the signature is the security
/// boundary, not the host prefix).
fn app_url() -> String {
    ConfigLoader::load()
        .ok()
        .and_then(|loader| AppConfig::from_loader(&loader).ok())
        .map(|config| config.url)
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| "http://localhost".to_string())
}

/// Resolve the process-wide [`SignedUrlSigner`], fail-closed.
///
/// Tests install an override through [`install_signer`]; production builds the
/// signer once from [`AppConfig::key`] via [`SignedUrlSigner::from_app_config`].
/// A blank/missing key yields `None`, so the caller answers `500` rather than
/// signing with a default key.
fn signer() -> Option<Arc<SignedUrlSigner>> {
    #[cfg(test)]
    if let Ok(slot) = SIGNER_OVERRIDE.get_or_init(|| RwLock::new(None)).read() {
        if let Some(signer) = slot.as_ref() {
            return Some(Arc::clone(signer));
        }
    }
    static BUILT: OnceLock<Option<Arc<SignedUrlSigner>>> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            ConfigLoader::load()
                .ok()
                .and_then(|loader| AppConfig::from_loader(&loader).ok())
                .and_then(|config| SignedUrlSigner::from_app_config(&config).ok())
                .map(Arc::new)
        })
        .clone()
}

/// Test-only signer override cell; see [`install_signer`].
#[cfg(test)]
static SIGNER_OVERRIDE: OnceLock<RwLock<Option<Arc<SignedUrlSigner>>>> = OnceLock::new();

/// Install (or clear) the test signer override.
///
/// The verify/resend handlers resolve the signer through [`signer`], so a test
/// that needs a deterministic key installs it here first. Serialize with the
/// shared provider lock, since the cell is process-wide.
#[cfg(test)]
pub(crate) fn install_signer(signer: Option<Arc<SignedUrlSigner>>) {
    let slot = SIGNER_OVERRIDE.get_or_init(|| RwLock::new(None));
    if let Ok(mut guard) = slot.write() {
        *guard = signer;
    }
}

/// Resolve the process-wide [`Mailer`], fail-closed.
///
/// Prefers a mailer already installed via [`Mail::set_mailer`] (the test
/// override); otherwise builds one once from `[mail]` config and installs it so
/// [`Mail::send`] can find it. An unbuildable transport yields `None` → `500`.
fn mailer() -> Option<Arc<dyn Mailer>> {
    if let Some(mailer) = Mail::mailer() {
        return Some(mailer);
    }
    static BUILT: OnceLock<Option<Arc<dyn Mailer>>> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let built = ConfigLoader::load()
                .ok()
                .and_then(|loader| MailConfig::from_loader(&loader).ok())
                .and_then(|config| mailer_from_config(&config).ok());
            if let Some(mailer) = &built {
                Mail::set_mailer(Arc::clone(mailer));
            }
            built
        })
        .clone()
}

/// Process-wide `verification.send` limiter registry.
///
/// The shared `[fortify.limiters]` registry only registers `login`,
/// `two-factor`, and `passkeys`; the kit's `verification.send` (6/minute) has no
/// analogue there, so this dedicated registry registers it once. It is
/// process-wide so its in-memory window accumulates across requests.
fn verification_registry() -> &'static RateLimiterRegistry {
    static REGISTRY: OnceLock<RateLimiterRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let registry = RateLimiterRegistry::new(Arc::new(MemoryRateLimiter::new()));
        registry.register(
            VERIFICATION_SEND,
            LimiterDefinition::new(Limit::per_minute(VERIFICATION_MAX_ATTEMPTS), |input| {
                input.session_id.or(input.username).map(str::to_string)
            }),
        );
        registry
    })
}
