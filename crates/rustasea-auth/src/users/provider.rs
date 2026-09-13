/// Async, write-capable user provider seam.
///
/// [`super::UserLookup`] is a synchronous, read-only credential resolver: it
/// is enough for the JWT/session guards' `login`, but it cannot create a user,
/// rotate a password hash, or flip `email_verified_at` — and it cannot await a
/// database round-trip. Registration, password reset, profile update, and
/// email verification all need to WRITE a user, and a database-backed login
/// needs to READ asynchronously. `UserProvider` is that seam.
///
/// # Relationship to `UserLookup`
///
/// `UserProvider` is deliberately **not** a supertrait of [`super::UserLookup`].
/// The two contracts serve different call sites (async DB writes vs. sync guard
/// reads), and forcing an async implementor to also satisfy the sync read
/// contract would leak an implementation detail. A type is free to implement
/// both — [`MemoryUserProvider`] does — but it is not required.
///
/// # Async convention
///
/// Every method returns a hand-rolled `Pin<Box<dyn Future<..> + Send>>` rather
/// than using `async fn` in the trait or `async_trait`, matching the crate's
/// existing [`crate::guard::Guard`] convention. This keeps the trait
/// object-safe (`&dyn UserProvider`) and `Send`.
///
/// # Fail-closed
///
/// The mutating methods have **no default implementation**: an implementor must
/// make an explicit choice for every write. [`DenyAllProvider`] is the
/// fail-closed default, mirroring [`super::StaticLookup`]'s posture.
///
/// # ORM adapter story
///
/// A database-backed implementation (e.g. over `rustasea-orm`) resolves each
/// call asynchronously inside its boxed future — `find_by_email` issues a
/// `SELECT`, `create` an `INSERT`, and so on. There is no pre-loading and no
/// feature gate: the async boundary is the trait itself, so the ORM adapter
/// returns its boxed query future directly. This crate stays DB-agnostic and
/// never depends on `sqlx`/`rustasea-orm`.
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::RwLock;

use uuid::Uuid;

use crate::error::{AuthError, Result};

use super::{AuthUserRecord, UserLookup};

/// Input for [`UserProvider::create`] (registration).
///
/// Mirrors the validated registration payload: the caller has already hashed
/// the plaintext password (`password_hash` is a PHC string) and decided the
/// initial `email_verified_at` (`None` for the typical "verify by email"
/// flow). `name` is carried for ORM-backed providers that persist a full
/// `users` row; it is not representable on [`AuthUserRecord`] (which stays
/// limited to the auth-relevant columns), so an in-memory provider retains
/// only the auth-relevant fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUserRecord {
    /// Display name.
    pub name: String,
    /// Unique login email.
    pub email: String,
    /// Argon2 PHC hash of the plaintext password.
    pub password_hash: String,
    /// `email_verified_at` to persist, or `None` for a fresh unverified user.
    pub email_verified_at: Option<String>,
}

impl NewUserRecord {
    /// Build an unverified registration record from an already-hashed password.
    pub fn new(
        name: impl Into<String>,
        email: impl Into<String>,
        password_hash: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            email: email.into(),
            password_hash: password_hash.into(),
            email_verified_at: None,
        }
    }

    /// Mark the record as already verified (builder form).
    pub fn with_email_verified_at(mut self, at: Option<impl Into<String>>) -> Self {
        self.email_verified_at = at.map(Into::into);
        self
    }
}

/// Async write+read contract for the auth features.
///
/// Sufficient for registration (`create`), password reset / change
/// (`update_password`), email verification and email-change re-verification
/// (`set_email_verified_at`), and database-backed login (`find_by_email`).
pub trait UserProvider: Send + Sync {
    /// Resolve the full user record for `email` (async DB-backed login read).
    ///
    /// Returns `Ok(None)` when no user matches; an un-wired or unavailable
    /// provider returns `Err`, never a silent `Ok(None)`.
    fn find_by_email<'a>(
        &'a self,
        email: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<AuthUserRecord>>> + Send + 'a>>;

    /// Persist a new user, returning the stored record (with its minted id).
    ///
    /// Must fail with [`AuthError::UserExists`] when `new_user.email` collides
    /// with an existing unique email.
    fn create<'a>(
        &'a self,
        new_user: NewUserRecord,
    ) -> Pin<Box<dyn Future<Output = Result<AuthUserRecord>> + Send + 'a>>;

    /// Replace the stored password hash for `user_id`.
    ///
    /// Must fail with [`AuthError::UserNotFound`] when `user_id` matches no
    /// user.
    fn update_password<'a>(
        &'a self,
        user_id: &'a str,
        password_hash: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

    /// Set (or clear) `email_verified_at` for `user_id`.
    ///
    /// Passing `None` clears the timestamp — used by an email change to force
    /// re-verification, mirroring Laravel's `markEmailAsUnverified`. Must fail
    /// with [`AuthError::UserNotFound`] when `user_id` matches no user.
    fn set_email_verified_at<'a>(
        &'a self,
        user_id: &'a str,
        verified_at: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

    /// Replace the stored display `name` and login `email` for `user_id`.
    ///
    /// Backs the profile-update route (`PATCH /settings/profile`): the caller
    /// has already authenticated and passes the desired values.
    ///
    /// Must fail with [`AuthError::UserNotFound`] when `user_id` matches no
    /// user, and with [`AuthError::UserExists`] when `email` is already owned by
    /// a *different* user (mirroring [`Self::create`]'s uniqueness rule).
    /// Re-asserting the caller's own current email is not a collision.
    ///
    /// [`AuthUserRecord`] does not carry `name`, so an in-memory provider keeps
    /// it in a side table (the read path does not expose it yet); a
    /// database-backed provider writes the `users.name` column directly.
    fn update_profile<'a>(
        &'a self,
        user_id: &'a str,
        name: &'a str,
        email: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;
}

/// Fail-closed provider: every operation is denied.
///
/// The default wiring posture until an application injects a real provider
/// (ADR-0007 explicit `AppState` wiring). Mirrors [`super::StaticLookup`]:
/// reads yield no credentials and writes are refused, so an un-wired app fails
/// closed rather than silently persisting or authenticating.
#[derive(Debug, Default)]
pub struct DenyAllProvider;

impl DenyAllProvider {
    /// Human-readable reason attached to every denial.
    const REASON: &'static str = "no user provider is wired; denying user access";
}

impl UserProvider for DenyAllProvider {
    fn find_by_email<'a>(
        &'a self,
        _email: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<AuthUserRecord>>> + Send + 'a>> {
        Box::pin(async move { Err(AuthError::Disabled(Self::REASON.to_string())) })
    }

    fn create<'a>(
        &'a self,
        _new_user: NewUserRecord,
    ) -> Pin<Box<dyn Future<Output = Result<AuthUserRecord>> + Send + 'a>> {
        Box::pin(async move { Err(AuthError::Disabled(Self::REASON.to_string())) })
    }

    fn update_password<'a>(
        &'a self,
        _user_id: &'a str,
        _password_hash: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move { Err(AuthError::Disabled(Self::REASON.to_string())) })
    }

    fn set_email_verified_at<'a>(
        &'a self,
        _user_id: &'a str,
        _verified_at: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move { Err(AuthError::Disabled(Self::REASON.to_string())) })
    }

    fn update_profile<'a>(
        &'a self,
        _user_id: &'a str,
        _name: &'a str,
        _email: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move { Err(AuthError::Disabled(Self::REASON.to_string())) })
    }
}

impl UserLookup for DenyAllProvider {
    fn hash_for_email(&self, _email: &str) -> Option<String> {
        None
    }

    fn id_for_email(&self, _email: &str) -> Option<String> {
        None
    }

    fn email_for_id(&self, _id: &str) -> Option<String> {
        None
    }
}

/// In-memory [`UserProvider`] for tests and local development.
///
/// **Test/dev only — not for production.** State lives in a process-local
/// `RwLock` map and is lost on restart: there is no durability, no cross-process
/// coordination, and no password policy. Use it to exercise registration,
/// reset, and verification flows without a database (e.g. from
/// `rustasea-testing`-style code), and inject a real provider in production.
///
/// Seeded like [`super::MemoryUserRegistry`] and additionally implements
/// [`UserLookup`], so one instance can drive both the sync `Guard::login` path
/// and the async [`crate::session::SessionGuard::login_with_provider`] path in
/// a single test.
#[derive(Debug, Default)]
pub struct MemoryUserProvider {
    by_email: RwLock<HashMap<String, AuthUserRecord>>,
    /// Display names keyed by user id.
    ///
    /// [`AuthUserRecord`] deliberately stays limited to the auth-relevant
    /// columns, so the in-memory provider keeps `name` (written by
    /// [`UserProvider::create`] / [`UserProvider::update_profile`]) in this
    /// side table. The read path does not expose it yet; use [`Self::name_for_id`].
    names: RwLock<HashMap<String, String>>,
}

impl MemoryUserProvider {
    /// Seed a user record directly (tests and local development).
    ///
    /// A poisoned lock leaves the provider unusable; fail closed (no-op)
    /// rather than panic in a framework crate.
    pub fn seed(&self, record: AuthUserRecord) {
        if let Ok(mut by_email) = self.by_email.write() {
            by_email.insert(record.email.clone(), record);
        }
    }

    /// Look up a record by email.
    pub fn by_email(&self, email: &str) -> Option<AuthUserRecord> {
        self.by_email.read().ok()?.get(email).cloned()
    }

    /// Look up a record by user id.
    pub fn by_id(&self, id: &str) -> Option<AuthUserRecord> {
        self.by_email
            .read()
            .ok()?
            .values()
            .find(|r| r.id == id)
            .cloned()
    }

    /// Resolve the stored display name for a user id.
    ///
    /// Returns `None` for an unknown id and for a record seeded without a name
    /// (a raw [`AuthUserRecord`] carries no `name`).
    pub fn name_for_id(&self, id: &str) -> Option<String> {
        self.names.read().ok()?.get(id).cloned()
    }
}

impl UserProvider for MemoryUserProvider {
    fn find_by_email<'a>(
        &'a self,
        email: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<AuthUserRecord>>> + Send + 'a>> {
        Box::pin(async move {
            let by_email = self
                .by_email
                .read()
                .map_err(|_| AuthError::StoreUnavailable)?;
            Ok(by_email.get(email).cloned())
        })
    }

    fn create<'a>(
        &'a self,
        new_user: NewUserRecord,
    ) -> Pin<Box<dyn Future<Output = Result<AuthUserRecord>> + Send + 'a>> {
        Box::pin(async move {
            let mut by_email = self
                .by_email
                .write()
                .map_err(|_| AuthError::StoreUnavailable)?;
            if by_email.contains_key(&new_user.email) {
                return Err(AuthError::UserExists {
                    email: new_user.email,
                });
            }
            let record = AuthUserRecord {
                id: Uuid::new_v4().to_string(),
                email: new_user.email,
                password_hash: new_user.password_hash,
                email_verified_at: new_user.email_verified_at,
            };
            by_email.insert(record.email.clone(), record.clone());
            if let Ok(mut names) = self.names.write() {
                names.insert(record.id.clone(), new_user.name);
            }
            Ok(record)
        })
    }

    fn update_password<'a>(
        &'a self,
        user_id: &'a str,
        password_hash: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut by_email = self
                .by_email
                .write()
                .map_err(|_| AuthError::StoreUnavailable)?;
            let record = by_email
                .values_mut()
                .find(|r| r.id == user_id)
                .ok_or_else(|| AuthError::UserNotFound {
                    id: user_id.to_string(),
                })?;
            record.password_hash = password_hash.to_string();
            Ok(())
        })
    }

    fn set_email_verified_at<'a>(
        &'a self,
        user_id: &'a str,
        verified_at: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut by_email = self
                .by_email
                .write()
                .map_err(|_| AuthError::StoreUnavailable)?;
            let record = by_email
                .values_mut()
                .find(|r| r.id == user_id)
                .ok_or_else(|| AuthError::UserNotFound {
                    id: user_id.to_string(),
                })?;
            record.email_verified_at = verified_at.map(str::to_string);
            Ok(())
        })
    }

    fn update_profile<'a>(
        &'a self,
        user_id: &'a str,
        name: &'a str,
        email: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut by_email = self
                .by_email
                .write()
                .map_err(|_| AuthError::StoreUnavailable)?;
            let old_email = by_email
                .values()
                .find(|r| r.id == user_id)
                .map(|r| r.email.clone())
                .ok_or_else(|| AuthError::UserNotFound {
                    id: user_id.to_string(),
                })?;
            // An email owned by a different user is a collision; re-asserting
            // the caller's own current email is not.
            if email != old_email && by_email.contains_key(email) {
                return Err(AuthError::UserExists {
                    email: email.to_string(),
                });
            }
            let mut record =
                by_email
                    .remove(&old_email)
                    .ok_or_else(|| AuthError::UserNotFound {
                        id: user_id.to_string(),
                    })?;
            record.email = email.to_string();
            by_email.insert(email.to_string(), record);
            if let Ok(mut names) = self.names.write() {
                names.insert(user_id.to_string(), name.to_string());
            }
            Ok(())
        })
    }
}

impl UserLookup for MemoryUserProvider {
    fn hash_for_email(&self, email: &str) -> Option<String> {
        self.by_email(email).map(|r| r.password_hash)
    }

    fn id_for_email(&self, email: &str) -> Option<String> {
        self.by_email(email).map(|r| r.id)
    }

    fn email_for_id(&self, id: &str) -> Option<String> {
        self.by_id(id).map(|r| r.email)
    }

    fn email_verified_at_for_id(&self, id: &str) -> Option<String> {
        self.by_id(id).and_then(|r| r.email_verified_at)
    }
}
