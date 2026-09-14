//! Process-wide wiring for the password-reset flow (AUTH-013).
//!
//! Split from [`super`] so each file stays under the 500-line cap. Everything
//! here resolves a shared, process-wide dependency once — the signing key, the
//! mailer, the token store, the broker's `expire`, the app URL, the limiter
//! registry, and the feature gate — with a **fail-closed** default: a value that
//! cannot be resolved yields `None`/a deny-all, never a permissive guess.
//!
//! Each `OnceLock` caches a value built from [`ConfigLoader`]; the test-only
//! overrides ([`install_signer`], [`install_reset_store`],
//! [`set_reset_passwords_disabled`]) let the integration tests inject
//! deterministic substitutes. Because the overrides are process-wide, the tests
//! serialize through the shared `PROVIDER_LOCK`.

use std::sync::{Arc, OnceLock, RwLock};

use rustasea::auth::{
    DenyAllResetStore, Limit, LimiterDefinition, MemoryRateLimiter, PasswordResetStore,
    RateLimiterRegistry, SignedUrlSigner,
};
use rustasea::foundation::AppConfig;
use rustasea::ConfigLoader;
use rustasea_mail::{
    mailer_from_config, Mail, MailConfig, MailMessage, Mailer, MinijinjaEngine, TemplateMailable,
    ViewEngine,
};
use serde::Serialize;

use crate::routes::helpers::fortify_config;

/// Default broker `expire` (minutes) when the config cannot be read (kit: 60).
const DEFAULT_EXPIRE_MINUTES: u64 = 60;

/// Named limiter for the request route (mirrors the `password.email` route name).
pub(super) const PASSWORD_EMAIL: &str = "password.email";

/// Request limit: 6 per minute per email. Laravel Breeze ships `throttle:6,1` on
/// the forgot-password route; the key is the submitted email (the kit's
/// `verification.send` keys on the submitted identity too), which is enough to
/// stop link spam without an IP dimension this app cannot resolve per-route.
const PASSWORD_EMAIL_MAX_ATTEMPTS: u32 = 6;

/// `500` detail when no signing key is configured (fail closed).
pub(super) const SIGNER_UNAVAILABLE: &str = "Password reset is temporarily unavailable.";
/// `500` detail when no mail transport can be resolved (fail closed).
pub(super) const MAILER_UNAVAILABLE: &str = "Email delivery is temporarily unavailable.";
/// `500` detail when the token store cannot be written (fail closed).
pub(super) const STORE_UNAVAILABLE: &str = "Password reset is temporarily unavailable.";

/// Whether password resets are disabled (test override included).
pub(super) fn reset_off() -> bool {
    #[cfg(test)]
    if RESET_OFF.load(std::sync::atomic::Ordering::SeqCst) {
        return true;
    }
    !fortify_config().features.reset_passwords
}

/// Test-only reset-password disable flag.
#[cfg(test)]
static RESET_OFF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Force the reset-password feature gate off (tests serialize via the lock).
#[cfg(test)]
pub(crate) fn set_reset_passwords_disabled(off: bool) {
    RESET_OFF.store(off, std::sync::atomic::Ordering::SeqCst);
}

/// The broker's `expire` in minutes (`[auth.passwords.<broker>].expire`).
///
/// Falls back to [`DEFAULT_EXPIRE_MINUTES`] when the config is unreadable or the
/// broker declares no positive expiry, so the flow never mints an immortal link.
pub(super) fn broker_expire_minutes() -> u64 {
    ConfigLoader::load()
        .ok()
        .and_then(|loader| rustasea::auth::AuthConfig::from_loader(&loader).ok())
        .and_then(|config| {
            let broker = config.defaults.passwords.clone();
            config.passwords.get(&broker).map(|entry| entry.expire)
        })
        .filter(|minutes| *minutes > 0)
        .unwrap_or(DEFAULT_EXPIRE_MINUTES)
}

/// The application base URL, used to make the signed link absolute.
pub(super) fn app_url() -> String {
    ConfigLoader::load()
        .ok()
        .and_then(|loader| AppConfig::from_loader(&loader).ok())
        .map(|config| config.url)
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| "http://localhost".to_string())
}

/// Resolve the process-wide [`SignedUrlSigner`], fail-closed.
pub(super) fn signer() -> Option<Arc<SignedUrlSigner>> {
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

/// Install (or clear) the test signer override (process-wide; serialize with the lock).
#[cfg(test)]
pub(crate) fn install_signer(signer: Option<Arc<SignedUrlSigner>>) {
    let slot = SIGNER_OVERRIDE.get_or_init(|| RwLock::new(None));
    if let Ok(mut guard) = slot.write() {
        *guard = signer;
    }
}

/// Resolve the process-wide [`Mailer`], fail-closed.
pub(super) fn mailer() -> Option<Arc<dyn Mailer>> {
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

/// Serializable context for the `mail/reset-password.html` template.
#[derive(Serialize)]
struct ResetPasswordContext {
    /// Absolute signed reset URL rendered into the body.
    link: String,
}

/// Runtime template engine for mail bodies, rooted at the shared views dir.
///
/// Mirrors the web layer's engine selection: the process-relative
/// [`rustasea::view::VIEWS_DIR`] wins when present (a deployed app / `cargo run`
/// from the workspace root); otherwise the workspace `resources/views` derived
/// from `CARGO_MANIFEST_DIR` is used, so `cargo test -p rustasea-app` (crate-root
/// cwd) renders the same templates.
fn mail_engine() -> Arc<dyn ViewEngine> {
    static ENGINE: OnceLock<Arc<dyn ViewEngine>> = OnceLock::new();
    Arc::clone(ENGINE.get_or_init(|| {
        let engine = if std::path::Path::new(rustasea::view::VIEWS_DIR).is_dir() {
            MinijinjaEngine::from_default_root()
        } else {
            MinijinjaEngine::new(crate::routes::resources_root().join("views"))
        };
        Arc::new(engine)
    }))
}

/// Render the reset email from `resources/views/mail/reset-password.html`.
///
/// Uses the shared runtime minijinja engine (AUTH-018), so the branded template
/// the app ships is the one it mails. A missing template or a render failure is
/// a typed [`rustasea_mail::MailError`], never an empty body.
pub(super) fn reset_password_message(
    to: String,
    link: String,
) -> rustasea_mail::Result<MailMessage> {
    TemplateMailable::new(
        mail_engine(),
        "mail/reset-password.html",
        ResetPasswordContext { link },
    )?
    .subject("Reset your password")
    .to(rustasea_mail::MailAddress::from_email(to))
    .try_build()
}

/// Process-wide [`PasswordResetStore`] cell, fail-closed to [`DenyAllResetStore`].
fn store_cell() -> &'static RwLock<Arc<dyn PasswordResetStore>> {
    static STORE: OnceLock<RwLock<Arc<dyn PasswordResetStore>>> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(Arc::new(DenyAllResetStore)))
}

/// Resolve the shared [`PasswordResetStore`] seam (fail-closed default).
pub(super) fn reset_store() -> Arc<dyn PasswordResetStore> {
    match store_cell().read() {
        Ok(store) => Arc::clone(&store),
        Err(_) => Arc::new(DenyAllResetStore),
    }
}

/// Test-only override of the process-wide [`PasswordResetStore`] seam.
#[cfg(test)]
pub(crate) fn install_reset_store(store: Arc<dyn PasswordResetStore>) {
    if let Ok(mut slot) = store_cell().write() {
        *slot = store;
    }
}

/// Process-wide `password.email` limiter registry (built once).
pub(super) fn reset_registry() -> &'static RateLimiterRegistry {
    static REGISTRY: OnceLock<RateLimiterRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let registry = RateLimiterRegistry::new(Arc::new(MemoryRateLimiter::new()));
        registry.register(
            PASSWORD_EMAIL,
            LimiterDefinition::new(Limit::per_minute(PASSWORD_EMAIL_MAX_ATTEMPTS), |input| {
                input.username.map(str::to_string)
            }),
        );
        registry
    })
}
