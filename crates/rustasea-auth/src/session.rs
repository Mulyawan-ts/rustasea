/// Session guard and session/cache hardening policy.
///
/// `SessionGuard` is the `tower-sessions` backed identity path: the store owns
/// the session record, the guard reads/writes the authenticated user through a
/// pluggable [`tower_sessions::SessionStore`] (in-memory by default). Login
/// always mints a **new** session id (session-fixation defense), `refresh`
/// cycles the id, and `logout` destroys the record and rotates the id.
///
/// Two drivers are selectable via `config/session.toml` `driver`:
///
/// - `memory` (default) — the per-process [`MemoryStore`], built by
///   [`SessionGuard::from_config`]. Sessions do not survive a restart.
/// - `database` — a [`DatabaseSessionStore`] over the ORM connection pool,
///   built by the async [`SessionGuard::from_config_database`] (needs a
///   [`rustasea_orm::ConnectionResolver`] to open the configured connection).
///   Sessions survive a restart; see the store's module doc for the operational
///   trade-offs (per-request round trip, required GC task, unencrypted payload).
///
/// Any other driver is rejected at config load time with a typed
/// [`crate::error::AuthConfigError::UnsupportedSessionDriver`].
///
/// `SessionPolicy` encodes the Laravel-13 hardening defaults: JSON
/// serialization, hyphenated `-session-`/`-cache-` key prefixes, and a
/// `serializable_classes` allow-list checked before any deserialization.
use std::sync::Arc;

use tower_sessions::session::{Id, Session};
use tower_sessions::{MemoryStore, SessionStore};

use crate::config::SessionConfig;
use crate::error::{AuthError, Result};
use crate::guard::{AuthUser, Credentials, Guard, Token};
use crate::session_cookie::SessionCookieConfig;
use crate::users::UserLookup;
use crate::verify::{Argon2Verifier, PasswordVerifier};

use rustasea_orm::ConnectionResolver;

/// Session lifetime advertised on issued tokens (two weeks, tower-sessions default).
///
/// Kept as the fallback when a guard is built without an explicit TTL (e.g.
/// [`SessionGuard::new`]); [`SessionGuard::with_ttl`] /
/// [`SessionGuard::from_config`] override it with the configured
/// `session.lifetime` (minutes → seconds).
const SESSION_TTL_SECS: u64 = 1_209_600;

mod database;
mod password;
mod policy;
mod provider;

pub use database::DatabaseSessionStore;
pub use policy::{DeserializationAllowList, SessionPolicy};

/// Identity stored inside a session for the session guard.
///
/// Carries the `users.email_verified_at` value so the `verified` gate can be
/// enforced from the session round-trip alone (no per-request database hit).
/// The password-confirmation timestamp is **not** part of the identity payload:
/// like Laravel's `auth.password_confirmed_at` it lives under a dedicated
/// session key ([`SessionPolicy::password_confirmed_key`]) and is read/written
/// by [`SessionGuard::confirm_password`] / [`SessionGuard::password_confirmed_at`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionUser {
    /// Authenticated user UUID.
    pub id: String,
    /// Optional display email.
    pub email: Option<String>,
    /// `users.email_verified_at` captured at login, or `None` when unverified.
    #[serde(default)]
    pub email_verified_at: Option<String>,
}

/// Session guard — resolves identity from a `tower-sessions` store.
///
/// The generic parameter is the backing store, defaulting to the in-memory
/// [`MemoryStore`]; any [`SessionStore`] implementation can be injected with
/// [`SessionGuard::with_store`]. The guard never issues bearer tokens — a
/// session cookie is the credential, and `access_token`/`refresh_token` carry
/// the session id so the HTTP layer can set the cookie.
///
/// # Stateless by design
///
/// The guard holds **no per-request mutable state**. A single instance is
/// registered as a shared `Arc<dyn Guard>` in [`crate::guard::AuthManager`]
/// and used concurrently by every Axum request; any `self`-mutating identity
/// cache would leak one request's principal into another. Identity therefore
/// lives exclusively in the store-backed session (keyed by the request's
/// session id) and is projected into request extensions
/// (`axum::Extension<AuthUser>`) by the HTTP middleware. `Guard::user`/`id`
/// consequently return `Ok(None)` — see their implementations.
pub struct SessionGuard<S: SessionStore = MemoryStore> {
    /// Guard name (`session`).
    name: String,
    /// Hardening policy applied to session payloads.
    policy: SessionPolicy,
    /// Pluggable backing store (in-memory by default).
    store: Arc<S>,
    /// Cookie hardening applied to the session cookie.
    cookie: SessionCookieConfig,
    /// Password verifier used by `login`.
    verifier: Arc<dyn PasswordVerifier>,
    /// Credential lookup used by `login` (fail-closed until DB wiring).
    lookup: Arc<dyn UserLookup>,
    /// Whether `login_using_id` is permitted (disabled by default).
    allow_login_using_id: bool,
    /// Access-token lifetime in seconds advertised by [`Guard::login`].
    ttl_secs: u64,
}

impl SessionGuard<MemoryStore> {
    /// Create a session guard backed by the in-memory store.
    pub fn new(policy: SessionPolicy) -> Self {
        Self::with_store(policy, Arc::new(MemoryStore::default()))
    }

    /// Build a guard from a typed [`SessionConfig`] over the in-memory store.
    ///
    /// Applies the configured cookie (name/secure/http_only/same_site/path/
    /// domain), the serialization policy (prefix derived from the cookie name),
    /// and the TTL (`lifetime` minutes × 60). The driver has already been
    /// validated as `memory` by [`SessionConfig::from_loader`].
    ///
    /// # Errors
    ///
    /// Propagates [`SessionConfig::to_policy`] / [`SessionConfig::to_cookie_config`]
    /// typed errors (invalid serialization, same_site, or cookie name).
    pub fn from_config(config: &SessionConfig) -> crate::config::ConfigResult<Self> {
        Self::new(SessionPolicy::default()).with_config(config)
    }
}

impl SessionGuard<DatabaseSessionStore> {
    /// Build a guard from a typed [`SessionConfig`] over the database store.
    ///
    /// Resolves the configured `session.connection` through `resolver`, opens a
    /// [`DatabaseSessionStore`] over the resulting pool, and applies the config's
    /// cookie, TTL, and serialization policy. Intended for
    /// `driver = "database"`; the memory driver uses the synchronous
    /// [`SessionGuard::from_config`] instead.
    ///
    /// # Errors
    ///
    /// [`AuthConfigError::SessionStoreUnavailable`] when the configured
    /// connection cannot be opened, or any
    /// [`SessionConfig::to_policy`] / [`SessionConfig::to_cookie_config`] typed
    /// error.
    pub async fn from_config_database(
        config: &SessionConfig,
        resolver: &ConnectionResolver,
    ) -> crate::config::ConfigResult<Self> {
        let store = Arc::new(DatabaseSessionStore::from_config(config, resolver).await?);
        Self::with_store(SessionPolicy::default(), store).with_config(config)
    }
}

impl<S: SessionStore> SessionGuard<S> {
    /// Apply a typed [`SessionConfig`]'s cookie, TTL, and serialization policy.
    ///
    /// The cookie and TTL are always applied. The policy is updated in place:
    /// `serialization` and `prefix` come from the config, while any
    /// `serializable_classes` allow-list already present on this guard is
    /// preserved (the config carries no allow-list, so it must never silently
    /// widen or clear an injected one).
    ///
    /// # Errors
    ///
    /// Propagates [`SessionConfig::to_policy`] / [`SessionConfig::to_cookie_config`]
    /// typed errors.
    pub fn with_config(mut self, config: &SessionConfig) -> crate::config::ConfigResult<Self> {
        let policy = config.to_policy()?;
        self.policy.serialization = policy.serialization;
        self.policy.prefix = policy.prefix;
        self.cookie = config.to_cookie_config()?;
        self.ttl_secs = config.ttl_secs();
        Ok(self)
    }

    /// Create a guard over an explicit store (Redis/SQLx/etc.).
    pub fn with_store(policy: SessionPolicy, store: Arc<S>) -> Self {
        Self {
            name: "session".to_string(),
            policy,
            store,
            cookie: SessionCookieConfig::default(),
            verifier: Arc::new(Argon2Verifier::new()),
            lookup: Arc::new(crate::users::StaticLookup),
            allow_login_using_id: false,
            ttl_secs: SESSION_TTL_SECS,
        }
    }

    /// Override the advertised session TTL (seconds).
    ///
    /// Wired from `session.lifetime` (minutes × 60) via
    /// [`SessionGuard::with_config`]; without this call the guard keeps the
    /// two-week [`SESSION_TTL_SECS`] fallback.
    pub fn with_ttl(mut self, ttl_secs: u64) -> Self {
        self.ttl_secs = ttl_secs;
        self
    }

    /// Attach the password verifier used by [`Guard::login`].
    pub fn with_verifier(mut self, verifier: Arc<dyn PasswordVerifier>) -> Self {
        self.verifier = verifier;
        self
    }

    /// Attach the credential lookup used by [`Guard::login`].
    pub fn with_lookup(mut self, lookup: Arc<dyn UserLookup>) -> Self {
        self.lookup = lookup;
        self
    }

    /// Enable or disable [`Guard::login_using_id`] (disabled by default).
    pub fn with_allow_login_using_id(mut self, allow: bool) -> Self {
        self.allow_login_using_id = allow;
        self
    }

    /// Override the session-cookie hardening configuration.
    pub fn with_cookie(mut self, cookie: SessionCookieConfig) -> Self {
        self.cookie = cookie;
        self
    }

    /// Hardening policy this guard enforces.
    pub fn policy(&self) -> &SessionPolicy {
        &self.policy
    }

    /// Session-cookie hardening applied to issued cookies.
    pub fn cookie(&self) -> &SessionCookieConfig {
        &self.cookie
    }

    /// Access-token lifetime (seconds) advertised by [`Guard::login`].
    pub fn ttl_secs(&self) -> u64 {
        self.ttl_secs
    }

    /// Shared handle to the backing store (HTTP wiring and tests).
    pub fn session_store(&self) -> Arc<S> {
        Arc::clone(&self.store)
    }

    /// Persist `user` under a freshly minted session id and return that id.
    ///
    /// A new id is always generated (`Session::new(None, ..)`), which is the
    /// session-fixation defense: a caller-supplied id is never adopted. This
    /// is the only place the guard writes identity — into the request-scoped
    /// session record, never into `self`.
    async fn persist(&self, user: &SessionUser) -> Result<Id> {
        let key = self.policy.user_key();
        let session = Session::new(None, Arc::clone(&self.store), None);
        session
            .insert(&key, user)
            .await
            .map_err(|_| AuthError::StoreUnavailable)?;
        session
            .save()
            .await
            .map_err(|_| AuthError::StoreUnavailable)?;
        session.id().ok_or(AuthError::StoreUnavailable)
    }

    /// Build the credential token for a session id.
    fn issue(&self, id: &Id) -> Token {
        let value = id.to_string();
        Token {
            access_token: value.clone(),
            refresh_token: value,
            token_type: "Session".to_string(),
            expires_in: self.ttl_secs,
        }
    }

    /// Parse a session id from a cookie/token string.
    fn parse_id(token: &str) -> Result<Id> {
        token.parse::<Id>().map_err(|_| AuthError::InvalidToken)
    }
}

impl<S: SessionStore> Guard for SessionGuard<S> {
    fn name(&self) -> &str {
        self.name.as_str()
    }

    fn login<'a>(
        &'a self,
        creds: &'a Credentials,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Token>> + Send + 'a>> {
        Box::pin(async move {
            let stored_hash = self
                .lookup
                .hash_for_email(&creds.email)
                .ok_or(AuthError::BadCredentials)?;
            // Constant-time argon2 verify; same error for unknown user and
            // wrong password so account enumeration via timing is not possible.
            if !self.verifier.verify(&stored_hash, &creds.password) {
                return Err(AuthError::BadCredentials);
            }
            let user_id = self
                .lookup
                .id_for_email(&creds.email)
                .ok_or(AuthError::BadCredentials)?;
            let email = self
                .lookup
                .email_for_id(&user_id)
                .or_else(|| Some(creds.email.clone()));
            let email_verified_at = self.lookup.email_verified_at_for_id(&user_id);
            let user = SessionUser {
                id: user_id,
                email,
                email_verified_at,
            };
            let id = self.persist(&user).await?;
            Ok(self.issue(&id))
        })
    }

    fn login_using_id<'a>(
        &'a self,
        user_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Token>> + Send + 'a>> {
        Box::pin(async move {
            if !self.allow_login_using_id {
                return Err(AuthError::Disabled(
                    "loginUsingId is disabled on the session guard".into(),
                ));
            }
            let user = SessionUser {
                id: user_id.to_string(),
                email: self.lookup.email_for_id(user_id),
                email_verified_at: self.lookup.email_verified_at_for_id(user_id),
            };
            let id = self.persist(&user).await?;
            Ok(self.issue(&id))
        })
    }

    fn parse<'a>(
        &'a self,
        token: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<AuthUser>> + Send + 'a>> {
        Box::pin(async move {
            let id = Self::parse_id(token)?;
            let key = self.policy.user_key();
            let session = Session::new(Some(id), Arc::clone(&self.store), None);
            let user: Option<SessionUser> = session
                .get(&key)
                .await
                .map_err(|_| AuthError::StoreUnavailable)?;
            let user = user.ok_or(AuthError::InvalidToken)?;
            // The password-confirmation timestamp lives under its own session
            // key (mirroring Laravel's `auth.password_confirmed_at`), so it is
            // read alongside the identity and projected onto the principal.
            let password_confirmed_at = self.read_password_confirmed_at(&session).await?;
            Ok(AuthUser {
                id: user.id,
                email: user.email,
                guard: self.name.clone(),
                email_verified_at: user.email_verified_at,
                password_confirmed_at,
            })
        })
    }

    fn refresh<'a>(
        &'a self,
        token: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Token>> + Send + 'a>> {
        Box::pin(async move {
            let id = Self::parse_id(token)?;
            let key = self.policy.user_key();
            let session = Session::new(Some(id), Arc::clone(&self.store), None);
            let user: Option<SessionUser> = session
                .get(&key)
                .await
                .map_err(|_| AuthError::StoreUnavailable)?;
            // The session must exist and carry the user payload; the record is
            // then re-saved under a fresh id (payload retained by `cycle_id`).
            user.ok_or(AuthError::InvalidToken)?;
            session
                .cycle_id()
                .await
                .map_err(|_| AuthError::StoreUnavailable)?;
            session
                .save()
                .await
                .map_err(|_| AuthError::StoreUnavailable)?;
            let new_id = session.id().ok_or(AuthError::StoreUnavailable)?;
            Ok(self.issue(&new_id))
        })
    }

    fn logout<'a>(
        &'a self,
        token: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let id = Self::parse_id(token)?;
            let session = Session::new(Some(id), Arc::clone(&self.store), None);
            // Destroy the stored record (no store write when the id is absent).
            // The old id is dead afterwards, so a replayed cookie can never
            // resume the authenticated session; the HTTP layer clears the
            // cookie on the response. No `self` state is touched.
            session
                .flush()
                .await
                .map_err(|_| AuthError::StoreUnavailable)?;
            Ok(())
        })
    }

    fn user<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<AuthUser>>> + Send + 'a>>
    {
        Box::pin(async move {
            // Stateless: the guard holds no per-request identity. A shared
            // guard is reused across concurrent requests, so resolving the
            // "current" user here would leak another request's principal.
            // The auth middleware projects `parse` results into
            // `Extension<AuthUser>` for the request scope instead — matching
            // `JwtGuard::user`, which also returns `Ok(None)`.
            Ok(None)
        })
    }

    fn id<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<String>>> + Send + 'a>>
    {
        Box::pin(async move {
            // See `user`: per-request identity is carried by request
            // extensions, never by shared guard state.
            Ok(None)
        })
    }
}

impl<S: SessionStore> std::fmt::Debug for SessionGuard<S> {
    /// Manual debug — the store is rendered by type, never by contents.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionGuard")
            .field("name", &self.name)
            .field("policy", &self.policy)
            .field("cookie", &self.cookie)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
