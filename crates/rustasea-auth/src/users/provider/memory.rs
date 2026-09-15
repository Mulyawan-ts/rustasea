/// In-memory [`UserProvider`] for tests and local development.
///
/// **Test/dev only — not for production.** State lives in a process-local
/// `RwLock` map and is lost on restart: there is no durability, no cross-process
/// coordination, and no password policy. Use it to exercise registration,
/// reset, and verification flows without a database (e.g. from
/// `rustasea-testing`-style code), and inject a real provider in production.
///
/// Seeded like [`super::super::MemoryUserRegistry`] and additionally implements
/// [`UserLookup`], so one instance can drive both the sync `Guard::login` path
/// and the async [`crate::session::SessionGuard::login_with_provider`] path in
/// a single test.
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::RwLock;

use uuid::Uuid;

use crate::error::{AuthError, Result};

use super::super::{AuthUserRecord, UserLookup};
use super::{NewUserRecord, UserProvider};

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

    fn find_by_id<'a>(
        &'a self,
        user_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<AuthUserRecord>>> + Send + 'a>> {
        Box::pin(async move { Ok(self.by_id(user_id)) })
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
                timezone: new_user.timezone,
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

    fn update_timezone<'a>(
        &'a self,
        user_id: &'a str,
        timezone: Option<String>,
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
            record.timezone = timezone;
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

    fn timezone_for_id(&self, id: &str) -> Option<String> {
        self.by_id(id).and_then(|r| r.timezone)
    }
}
