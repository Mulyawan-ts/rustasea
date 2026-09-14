//! Two-factor persistence seam + in-memory / fail-closed implementations.
//!
//! RustaSea has no database-backed two-factor columns wired yet, so the
//! persistence boundary is an explicit trait — [`TwoFactorStore`] — exactly as
//! user writes go through [`crate::users::UserProvider`]. A database-backed
//! implementation (over `rustasea-orm`) resolves each call inside its boxed
//! future; this crate stays DB-agnostic.
//!
//! # Async convention
//!
//! Like [`crate::users::UserProvider`], each method returns a hand-rolled
//! `Pin<Box<dyn Future<..> + Send>>` rather than `async fn`, keeping the trait
//! object-safe (`&dyn TwoFactorStore`) and `Send`.
//!
//! # Fail closed
//!
//! The mutating methods have **no default implementation**: an implementor must
//! make an explicit choice for every write. [`DenyAllTwoFactorStore`] is the
//! fail-closed default (mirrors [`crate::users::DenyAllProvider`]); an un-wired
//! or unavailable store returns `Err`, never a silent `Ok`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::RwLock;

use crate::error::{AuthError, Result};

/// One user's two-factor state (the `users` 2FA columns).
///
/// `secret` is an encrypted envelope (never the base32 secret in the clear) and
/// `recovery_codes` holds Argon2 PHC hashes (never the codes themselves), so a
/// store compromise does not hand an attacker a working second factor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TwoFactorRecord {
    /// Owning user UUID.
    pub user_id: String,
    /// Sealed (encrypted) base32 TOTP secret, or `None` before enable.
    pub secret: Option<String>,
    /// Argon2 PHC hashes of the recovery codes.
    pub recovery_codes: Vec<String>,
    /// RFC 3339 confirmation timestamp, or `None` while unconfirmed.
    pub confirmed_at: Option<String>,
}

impl TwoFactorRecord {
    /// Whether two-factor authentication has been confirmed for this user.
    pub fn is_confirmed(&self) -> bool {
        self.confirmed_at.is_some()
    }
}

/// Async persistence contract for two-factor state.
pub trait TwoFactorStore: Send + Sync {
    /// Resolve the record for `user_id`.
    ///
    /// `Ok(None)` means no two-factor state exists; an un-wired or unavailable
    /// store returns `Err`, never a silent `Ok(None)`.
    fn get<'a>(
        &'a self,
        user_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<TwoFactorRecord>>> + Send + 'a>>;

    /// Upsert the record for `record.user_id`.
    fn put<'a>(
        &'a self,
        record: TwoFactorRecord,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

    /// Delete the record for `user_id` (disable two-factor authentication).
    ///
    /// Deleting an absent record is not an error — the post-condition is "no
    /// two-factor state for this user".
    fn delete<'a>(
        &'a self,
        user_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;
}

/// Fail-closed store: every operation is denied.
///
/// The default wiring posture until an application injects a real store.
#[derive(Debug, Default)]
pub struct DenyAllTwoFactorStore;

impl DenyAllTwoFactorStore {
    /// Human-readable reason attached to every denial.
    const REASON: &'static str = "no two-factor store is wired; denying access";
}

impl TwoFactorStore for DenyAllTwoFactorStore {
    fn get<'a>(
        &'a self,
        _user_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<TwoFactorRecord>>> + Send + 'a>> {
        Box::pin(async move { Err(AuthError::Disabled(Self::REASON.to_string())) })
    }

    fn put<'a>(
        &'a self,
        _record: TwoFactorRecord,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move { Err(AuthError::Disabled(Self::REASON.to_string())) })
    }

    fn delete<'a>(
        &'a self,
        _user_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move { Err(AuthError::Disabled(Self::REASON.to_string())) })
    }
}

/// In-memory [`TwoFactorStore`] for tests and local development.
///
/// **Test/dev only — not for production.** State lives in a process-local
/// `RwLock` map and is lost on restart; inject a real store in production.
#[derive(Debug, Default)]
pub struct MemoryTwoFactorStore {
    /// Records keyed by user id.
    records: RwLock<HashMap<String, TwoFactorRecord>>,
}

impl MemoryTwoFactorStore {
    /// Seed a record directly (tests and local development).
    ///
    /// A poisoned lock leaves the store unusable; fail closed (no-op) rather
    /// than panic in a framework crate.
    pub fn seed(&self, record: TwoFactorRecord) {
        if let Ok(mut records) = self.records.write() {
            records.insert(record.user_id.clone(), record);
        }
    }

    /// Read a record synchronously (test assertions).
    pub fn get_sync(&self, user_id: &str) -> Option<TwoFactorRecord> {
        self.records.read().ok()?.get(user_id).cloned()
    }
}

impl TwoFactorStore for MemoryTwoFactorStore {
    fn get<'a>(
        &'a self,
        user_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<TwoFactorRecord>>> + Send + 'a>> {
        Box::pin(async move {
            let records = self
                .records
                .read()
                .map_err(|_| AuthError::StoreUnavailable)?;
            Ok(records.get(user_id).cloned())
        })
    }

    fn put<'a>(
        &'a self,
        record: TwoFactorRecord,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut records = self
                .records
                .write()
                .map_err(|_| AuthError::StoreUnavailable)?;
            records.insert(record.user_id.clone(), record);
            Ok(())
        })
    }

    fn delete<'a>(
        &'a self,
        user_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut records = self
                .records
                .write()
                .map_err(|_| AuthError::StoreUnavailable)?;
            records.remove(user_id);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The in-memory store round-trips put/get/delete.
    #[tokio::test]
    async fn memory_store_round_trip() {
        let store = MemoryTwoFactorStore::default();
        assert!(store.get("u1").await.expect("get").is_none());

        store
            .put(TwoFactorRecord {
                user_id: "u1".to_string(),
                secret: Some("sealed".to_string()),
                recovery_codes: vec!["hash".to_string()],
                confirmed_at: Some("2026-01-01T00:00:00Z".to_string()),
            })
            .await
            .expect("put");
        let record = store.get("u1").await.expect("get").expect("present");
        assert!(record.is_confirmed());

        store.delete("u1").await.expect("delete");
        assert!(store.get("u1").await.expect("get").is_none());
        // Deleting an absent record is idempotent.
        store.delete("u1").await.expect("second delete");
    }

    /// The deny-all store fails every operation closed.
    #[tokio::test]
    async fn deny_all_store_denies() {
        let store = DenyAllTwoFactorStore;
        assert!(store.get("u1").await.is_err());
        assert!(store.put(TwoFactorRecord::default()).await.is_err());
        assert!(store.delete("u1").await.is_err());
    }
}
