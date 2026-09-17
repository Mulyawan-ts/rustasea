//! Settings routes — profile, password, and security management.
//!
//! `/settings` redirects to `/settings/profile`. The profile `GET` renders a
//! minimal placeholder; the profile `PATCH` (`profile.update`) validates and
//! persists name/email changes against the app's [`UserProvider`]. The password
//! `PUT` validates `current_password` + a policy-compliant new password and
//! persists the new hash. `/settings/security` is gated by the
//! `password.confirm` middleware id registered in [`super`].
//!
//! [`UserProvider`]: rustasea::auth::UserProvider

use axum::extract::Extension;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};

use rustasea::auth::users::AuthUserRecord;
use rustasea::auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea::auth::{AuthError, AuthUser};
use rustasea::router::Router as RouteTable;
use rustasea::validation::serde_json::{Map, Value};
use rustasea::validation::{ErrorBag, PasswordPolicy, Rules, ValidationContext, ValidationError};

use super::helpers::{field, fortify_config, json_error, parse_form, see_other, user_provider};
use super::{web, LOGIN_PATH, PASSWORD_CONFIRM};

/// Profile settings page markup.
///
/// Fallback only: [`profile_page`] renders `resources/views/settings/profile.html`
/// through the shared minijinja engine and serves this structured document when
/// the template is unavailable (a missing file or a render error).
const PROFILE_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Profile settings</title></head>
<body><main><h1>Profile settings</h1>
<p>Update your name and email address here.</p>
</main></body></html>"#;

/// Password settings page markup.
///
/// Fallback only: [`password_page`] renders `resources/views/settings/password.html`
/// through the shared minijinja engine and serves this structured document when
/// the template is unavailable.
const PASSWORD_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Password settings</title></head>
<body><main><h1>Password settings</h1>
<p>Change your account password here.</p>
</main></body></html>"#;

/// Security settings page markup (gated).
///
/// Fallback only: [`security_page`] renders `resources/views/settings/security.html`
/// through the shared minijinja engine and serves this structured document when
/// the template is unavailable.
const SECURITY_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Security settings</title></head>
<body><main><h1>Security settings</h1>
<p>Two-factor authentication and passkeys appear here.</p>
</main></body></html>"#;

/// Redirect target after a successful profile update (`303 See Other`).
const PROFILE_PATH: &str = "/settings/profile";

/// Redirect target after a successful password update (`303 See Other`).
const PASSWORD_PATH: &str = "/settings/password";

/// Detail for a `500` when the user provider cannot be resolved (fail closed).
const PROFILE_UNAVAILABLE: &str = "Profile updates are temporarily unavailable.";

/// Detail for a `500` when a password update cannot be hashed or persisted.
const PASSWORD_UNAVAILABLE: &str = "Password updates are temporarily unavailable.";

/// Register the settings route table onto `table`.
///
/// `/settings/security` is registered inside a [`RouteTable::group`] because
/// [`RouteTable::middleware`] is sticky — its pending middleware applies to
/// every later route on the same table, so the group scopes the
/// `password.confirm` gate and keeps it off the console table.
pub fn register(table: &mut RouteTable) {
    table.redirect("/settings", PROFILE_PATH);
    table
        .get_action(PROFILE_PATH, profile_page)
        .named("profile.edit");
    table
        .patch_action(PROFILE_PATH, profile_update)
        .named("profile.update");
    table
        .get_action("/settings/password", password_page)
        .named("password.edit");
    table.put_action("/settings/password", password_update);
    table.group(|group| {
        group
            .middleware(PASSWORD_CONFIRM)
            .get_action("/settings/security", security_page)
            .named("security.edit");
    });
}

/// GET /settings/profile — render the profile form.
///
/// Renders `resources/views/settings/profile.html` through the shared engine
/// ([`web::render_view`]) so the page carries the app shell and the optional
/// authenticated principal is exposed to the template as `user`. When the
/// template cannot be loaded or rendered the structured [`PROFILE_HTML`]
/// fallback is served instead, so the route always answers HTML rather than a
/// `500`.
async fn profile_page(user: Option<Extension<AuthUser>>) -> Response {
    render_or_fallback(
        "settings/profile.html",
        user.map(|Extension(user)| user),
        PROFILE_HTML,
    )
}

/// PATCH /settings/profile — validate and persist name/email changes.
///
/// Flow: (1) require an authenticated principal ([`Extension<AuthUser>`]);
/// absent → `302` → `/login` (never a `401`); (2) parse the urlencoded body by
/// hand (see [`parse_form`], the same `%XX`/`+` decoding `auth.rs` uses, no body
/// extractor); (3) validate `name`/`email` against a [`Rules`] set whose
/// `unique:users,email` probe is answered by [`ProviderContext`] with the
/// current user id as the self-exclusion `ignore_id` — a payload that omits a
/// field is built as a JSON object *without* that key so `required` fires rather
/// than the rule seeing `""`; (4) persist via [`UserProvider::update_profile`];
/// (5) if the submitted email changed **and**
/// `[fortify].features.email_verification` is on, clear `email_verified_at` so
/// the new address must be re-verified (Laravel `markEmailAsUnverified`
/// parity); (6) `303 See Other` → `/settings/profile` so the browser re-GETs.
///
/// Status codes: `303` success; `422` a validation failure **or** a persisted
/// email collision ([`AuthError::UserExists`], e.g. a race the pre-check
/// missed); `500` the provider could not be resolved or a store write failed
/// (fail closed, never a silent success).
///
/// Fail-closed: the `unique` probe returns [`None`] when the provider errors,
/// so validation fails closed with a typed `missing_context` entry rather than
/// passing the email through; the re-verification step runs **after** the
/// profile write, so if it fails the handler answers `500` rather than claiming
/// success (the two provider calls are not one transaction — a partial success
/// surfaced explicitly); when the guard exposes no current email
/// (`AuthUser::email` is `None`) the submitted address is treated as *changed*,
/// so verification is cleared — the conservative choice.
///
/// [`UserProvider`]: rustasea::auth::UserProvider
async fn profile_update(user: Option<Extension<AuthUser>>, body: axum::body::Bytes) -> Response {
    let Some(Extension(user)) = user else {
        return found(LOGIN_PATH);
    };

    let fields = parse_form(&body);
    let name = field(&fields, "name").unwrap_or_default().to_string();
    let email = field(&fields, "email").unwrap_or_default().to_string();

    // Build the payload so a *missing* field is an absent key (fails
    // `required`) rather than an empty string.
    let mut payload = Map::new();
    if let Some(value) = field(&fields, "name") {
        payload.insert("name".to_string(), Value::String(value.to_string()));
    }
    if let Some(value) = field(&fields, "email") {
        payload.insert("email".to_string(), Value::String(value.to_string()));
    }
    let payload = Value::Object(payload);

    // Resolve the `unique` probe before validation. The `ValidationContext`
    // trait is synchronous but the provider is async, and a sync method cannot
    // await, so the single lookup the rule needs is awaited here and handed to
    // the adapter as a snapshot. A lookup error becomes `Unavailable`, which
    // makes `is_unique` return `None` and the rule fail closed.
    let provider = user_provider();
    let probe = match provider.find_by_email(&email).await {
        Ok(found) => UniqueProbe::Resolved(found),
        Err(_) => UniqueProbe::Unavailable,
    };
    let context = ProviderContext {
        queried_email: email.clone(),
        probe,
        current_password_match: None,
    };

    let user_id = user.id.as_str();
    let email_rules = format!("required|string|email|max:255|unique:users,email,{user_id}");
    let rules = Rules::new()
        .field("name", "required|string|max:255")
        .field("email", &email_rules);
    if let Err(bag) = rules.validate_with(&payload, &context) {
        return validation_error(bag);
    }

    match provider.update_profile(&user.id, &name, &email).await {
        Ok(()) => {}
        // A collision the pre-check missed (a concurrent write): report it as a
        // field error on `email`, the same shape as a `unique` failure.
        Err(AuthError::UserExists { .. }) => {
            return field_error("email", "unique", "The email has already been taken.");
        }
        // The authenticated user vanished, or the store is unreachable: fail
        // closed with a `500`, never a silent success.
        Err(_) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "AuthError::Unavailable",
                PROFILE_UNAVAILABLE,
            );
        }
    }

    // Email-change re-verification (kit parity): only when the address actually
    // changed and the feature is enabled. An unchanged email is left untouched.
    let email_changed = user.email.as_deref() != Some(email.as_str());
    if email_changed
        && fortify_config().features.email_verification
        && provider
            .set_email_verified_at(&user.id, None)
            .await
            .is_err()
    {
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "AuthError::Unavailable",
            PROFILE_UNAVAILABLE,
        );
    }

    see_other(PROFILE_PATH)
}

/// GET /settings/password — render the password form.
///
/// Renders `resources/views/settings/password.html` through the shared engine
/// ([`web::render_view`]); when the template is unavailable the structured
/// [`PASSWORD_HTML`] fallback is served instead.
async fn password_page(user: Option<Extension<AuthUser>>) -> Response {
    render_or_fallback(
        "settings/password.html",
        user.map(|Extension(user)| user),
        PASSWORD_HTML,
    )
}

/// PUT /settings/password — validate the current password and a
/// policy-compliant new one, then persist the new Argon2 hash.
///
/// Requires an authenticated principal (absent → `302` → `/login`). The
/// submitted `current_password` is checked against the stored hash through the
/// pre-await snapshot in [`ProviderContext`]; the new `password` must satisfy
/// [`PasswordPolicy::production`] and be `confirmed`. A validation failure →
/// `422` with the [`ErrorBag`]; a hashing/provider failure → `500` (fail
/// closed); success → `303 See Other` → [`PASSWORD_PATH`]. The session is left
/// **valid** after the change (kit parity — it neither rotates nor invalidates
/// the session, so the caller stays logged in).
async fn password_update(user: Option<Extension<AuthUser>>, body: axum::body::Bytes) -> Response {
    let Some(Extension(user)) = user else {
        return found(LOGIN_PATH);
    };

    let fields = parse_form(&body);
    let current = field(&fields, "current_password").unwrap_or_default();
    let password = field(&fields, "password").unwrap_or_default().to_string();

    // Pre-await snapshot: the sync `current_password_matches` cannot await, so
    // resolve the stored hash here and verify it. A missing email, provider
    // error, unknown user, or absent hash all fail closed (`None` → the rule
    // yields a typed `missing_context`, never a silent pass).
    let provider = user_provider();
    let current_match = match user.email.as_deref() {
        Some(email) => match provider.find_by_email(email).await {
            Ok(Some(record)) => Some(Argon2Verifier::new().verify(&record.password_hash, current)),
            Ok(None) | Err(_) => None,
        },
        None => None,
    };
    let context = ProviderContext {
        queried_email: String::new(),
        probe: UniqueProbe::Unavailable,
        current_password_match: current_match,
    };

    let mut payload = Map::new();
    for key in ["current_password", "password", "password_confirmation"] {
        if let Some(value) = field(&fields, key) {
            payload.insert(key.to_string(), Value::String(value.to_string()));
        }
    }
    let rules = Rules::new()
        .password_policy(PasswordPolicy::production())
        .field("current_password", "required|string|current_password")
        .field("password", "required|string|password|confirmed");
    if let Err(bag) = rules.validate_with(&Value::Object(payload), &context) {
        return validation_error(bag);
    }

    let Ok(hash) = Argon2Verifier::new().hash(&password) else {
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "AuthError::Hash",
            PASSWORD_UNAVAILABLE,
        );
    };
    match provider.update_password(&user.id, &hash).await {
        Ok(()) => see_other(PASSWORD_PATH),
        Err(_) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "AuthError::Unavailable",
            PASSWORD_UNAVAILABLE,
        ),
    }
}

/// GET /settings/security — render the security page behind the gate.
///
/// Renders `resources/views/settings/security.html` through the shared engine
/// ([`web::render_view`]); when the template is unavailable the structured
/// [`SECURITY_HTML`] fallback is served instead.
async fn security_page(user: Option<Extension<AuthUser>>) -> Response {
    render_or_fallback(
        "settings/security.html",
        user.map(|Extension(user)| user),
        SECURITY_HTML,
    )
}

/// Render `template` with the optional authenticated principal, falling back to
/// `fallback` when the template cannot be loaded or rendered.
///
/// The fallback keeps the route total: a deployment that ships without the
/// runtime views directory still answers a structured HTML document rather than
/// surfacing the engine's `500`. The `user` projection and engine selection are
/// shared with [`web::render_view`], so the fallback is the only divergence.
fn render_or_fallback(template: &str, user: Option<AuthUser>, fallback: &'static str) -> Response {
    match web::render_view(template, user) {
        Ok(response) => response.into_response(),
        Err(_) => Html(fallback).into_response(),
    }
}

/// Build a `302 Found` redirect to `location` (the unauthenticated case); an
/// invalid location falls back to `/` rather than panicking.
fn found(location: &str) -> Response {
    let value = HeaderValue::try_from(location).unwrap_or_else(|_| HeaderValue::from_static("/"));
    (StatusCode::FOUND, [(header::LOCATION, value)]).into_response()
}

/// Build a `422` response carrying an [`ErrorBag`] in the documented shape
/// (`{"message": "...", "errors": { "<field>": ["..."] }}`, `api-validation.md`).
fn validation_error(bag: ErrorBag) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        axum::Json(bag.into_json_body()),
    )
        .into_response()
}

/// Build a `422` response with a single field error in the same shape as
/// [`validation_error`].
fn field_error(field: &str, code: &str, message: &str) -> Response {
    let mut bag = ErrorBag::new();
    bag.add(field, ValidationError::new(code, message));
    validation_error(bag)
}

/// Snapshot of the email-uniqueness probe resolved before validation runs:
/// `Resolved(Some(record))` = a user owns the email, `Resolved(None)` = free,
/// `Unavailable` = the provider errored (unknown).
enum UniqueProbe {
    /// The provider answered (the owning record, or `None` when free).
    Resolved(Option<AuthUserRecord>),
    /// The provider errored — the answer cannot be determined.
    Unavailable,
}

/// A [`ValidationContext`] adapter over the app's async [`UserProvider`].
///
/// [`ValidationContext`] is synchronous and object-safe while [`UserProvider`]
/// is asynchronous, and a sync `is_unique` cannot `.await` (blocking inside a
/// handler would risk a runtime deadlock). The adapter therefore carries the
/// probe(s) the rules need — resolved by the handler *before* validation — and
/// answers from that snapshot.
///
/// Fail-closed: `is_unique` returns `None` (→ the `unique` rule fails with a
/// typed `missing_context`) whenever the probe is [`UniqueProbe::Unavailable`],
/// the table/column is not backed (`users`/`email` only), or the value differs
/// from the resolved one; `None` is never a silent pass.
///
/// [`UserProvider`]: rustasea::auth::UserProvider
struct ProviderContext {
    /// The email the probe was resolved for; other values fail closed.
    queried_email: String,
    /// The resolved ownership answer.
    probe: UniqueProbe,
    /// Snapshot of the `current_password` comparison resolved before
    /// validation (see [`password_update`]); `None` fails the rule closed.
    current_password_match: Option<bool>,
}

impl ValidationContext for ProviderContext {
    /// Whether the submitted email is free, mirroring Laravel's
    /// `Rule::unique('users', 'email')->ignore($userId)`: `Some(true)` free or
    /// the caller's own row (`ignore_id` matches the found id); `Some(false)`
    /// owned by another user; `None` cannot determine (fails closed).
    fn is_unique(
        &self,
        table: &str,
        column: &str,
        value: &str,
        ignore_id: Option<&str>,
    ) -> Option<bool> {
        if table != "users" || column != "email" || value != self.queried_email {
            return None;
        }
        match &self.probe {
            UniqueProbe::Unavailable => None,
            UniqueProbe::Resolved(None) => Some(true),
            UniqueProbe::Resolved(Some(record)) => Some(ignore_id == Some(record.id.as_str())),
        }
    }

    /// Answer from the pre-await snapshot taken by [`password_update`]: the sync
    /// trait method cannot await, so the handler verifies the stored hash before
    /// validation and stashes the boolean. `None` (no authenticated email,
    /// provider error, unknown user, or no hash) fails the rule closed.
    fn current_password_matches(&self, _plaintext: &str) -> Option<bool> {
        self.current_password_match
    }
}
