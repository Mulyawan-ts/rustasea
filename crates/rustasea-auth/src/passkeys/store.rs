//! Passkey credential persistence seam + in-memory / fail-closed implementations.
//!
//! RustaSea has no database-backed passkey table wired yet, so the persistence
//! boundary is an explicit trait — [`PasskeyStore`] — exactly as user writes go
//! through [`crate::users::UserProvider`]. A database-backed implementation
//! (over `rustasea-orm`) resolves each call inside its boxed future; this crate
//! stays DB-agnostic.
//!
//! # What is stored
//!
//! Only the **public** key material is persisted — never a private key, and
//! never a challenge. `public_key` is the SEC1 uncompressed encoding of the
//! credential's P-256 point (`0x04 || x || y`); `counter` is the signature
//! counter the authenticator last reported, used to detect cloned credentials.
//!
//! # Async convention
//!
//! Like [`crate::users::UserProvider`], each method returns a hand-rolled
//! `Pin<Box<dyn Future<..> + Send>>` rather than `async fn`, keeping the trait
//! object-safe (`&dyn PasskeyStore`) and `Send`.
//!
//! # Fail closed
//!
//! The mutating methods have **no default implementation**: an implementor must
//! make an explicit choice for every write. [`DenyAllPasskeyStore`] is the
//! fail-closed default (mirrors [`crate::users::DenyAllProvider`]); an un-wired
//! or unavailable store returns `Err`, never a silent `Ok`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::RwLock;

use crate::error::{AuthError, Result};

/// One registered passkey credential.
///
/// The `id` is the WebAuthn credential id (base64url, as the authenticator
/// reports it) and is globally unique; `user_id` binds it to the owning account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasskeyCredential {
    /// WebAuthn credential id (base64url).
    pub id: String,
    /// Owning user UUID.
    pub user_id: String,
    /// SEC1 uncompressed P-256 public key (`0x04 || x || y`, 65 bytes).
    pub public_key: Vec<u8>,
    /// Signature counter reported by the authenticator at registration.
    pub counter: u32,
    /// Human-readable label the user assigned to this credential.
    pub name: String,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
}

/// Async persistence contract for passkey credentials.
pub trait PasskeyStore: Send + Sync {
    /// Persist a newly registered credential.
    ///
    /// Must fail with [`AuthError::Passkey`] when `credential.id` already
    /// exists (a credential id is globally unique).
    fn create<'a>(
        &'a self,
        credential: PasskeyCredential,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

    /// Resolve a credential by its WebAuthn credential id.
    ///
    /// `Ok(None)` means no such credential; an un-wired or unavailable store
    /// returns `Err`, never a silent `Ok(None)`.
    fn find<'a>(
        &'a self,
        credential_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<PasskeyCredential>>> + Send + 'a>>;

    /// List every credential owned by `user_id`.
    fn list<'a>(
        &'a self,
        user_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<PasskeyCredential>>> + Send + 'a>>;

    /// Delete the credential `credential_id` **owned by** `user_id`.
    ///
    /// Ownership is part of the contract so one account can never delete
    /// another's credential. Deleting an absent (or not-owned) credential is
    /// not an error — the post-condition is "this user no longer owns it".
    fn delete<'a>(
        &'a self,
        user_id: &'a str,
        credential_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

    /// Advance the stored signature counter for `credential_id`.
    fn update_counter<'a>(
        &'a self,
        credential_id: &'a str,
        counter: u32,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;
}

/// Fail-closed store: every operation is denied.
///
/// The default wiring posture until an application injects a real store.
#[derive(Debug, Default)]
pub struct DenyAllPasskeyStore;

impl DenyAllPasskeyStore {
    /// Human-readable reason attached to every denial.
    const REASON: &'static str = "no passkey store is wired; denying access";
}

impl PasskeyStore for DenyAllPasskeyStore {
    fn create<'a>(
        &'a self,
        _credential: PasskeyCredential,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move { Err(AuthError::Disabled(Self::REASON.to_string())) })
    }

    fn find<'a>(
        &'a self,
        _credential_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<PasskeyCredential>>> + Send + 'a>> {
        Box::pin(async move { Err(AuthError::Disabled(Self::REASON.to_string())) })
    }

    fn list<'a>(
        &'a self,
        _user_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<PasskeyCredential>>> + Send + 'a>> {
        Box::pin(async move { Err(AuthError::Disabled(Self::REASON.to_string())) })
    }

    fn delete<'a>(
        &'a self,
        _user_id: &'a str,
        _credential_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move { Err(AuthError::Disabled(Self::REASON.to_string())) })
    }

    fn update_counter<'a>(
        &'a self,
        _credential_id: &'a str,
        _counter: u32,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move { Err(AuthError::Disabled(Self::REASON.to_string())) })
    }
}

/// In-memory [`PasskeyStore`] for tests and local development.
///
/// **Test/dev only — not for production.** State lives in a process-local
/// `RwLock` map and is lost on restart; inject a real store in production.
#[derive(Debug, Default)]
pub struct MemoryPasskeyStore {
    /// Credentials keyed by credential id.
    credentials: RwLock<HashMap<String, PasskeyCredential>>,
}

impl MemoryPasskeyStore {
    /// Seed a credential directly (tests and local development).
    ///
    /// A poisoned lock leaves the store unusable; fail closed (no-op) rather
    /// than panic in a framework crate.
    pub fn seed(&self, credential: PasskeyCredential) {
        if let Ok(mut credentials) = self.credentials.write() {
            credentials.insert(credential.id.clone(), credential);
        }
    }

    /// Read one credential synchronously (test assertions).
    pub fn get_sync(&self, credential_id: &str) -> Option<PasskeyCredential> {
        self.credentials.read().ok()?.get(credential_id).cloned()
    }

    /// List a user's credentials synchronously (test assertions).
    pub fn list_sync(&self, user_id: &str) -> Vec<PasskeyCredential> {
        self.credentials
            .read()
            .map(|credentials| {
                credentials
                    .values()
                    .filter(|credential| credential.user_id == user_id)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl PasskeyStore for MemoryPasskeyStore {
    fn create<'a>(
        &'a self,
        credential: PasskeyCredential,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut credentials = self
                .credentials
                .write()
                .map_err(|_| AuthError::StoreUnavailable)?;
            if credentials.contains_key(&credential.id) {
                return Err(AuthError::Passkey("credential already registered".into()));
            }
            credentials.insert(credential.id.clone(), credential);
            Ok(())
        })
    }

    fn find<'a>(
        &'a self,
        credential_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<PasskeyCredential>>> + Send + 'a>> {
        Box::pin(async move {
            let credentials = self
                .credentials
                .read()
                .map_err(|_| AuthError::StoreUnavailable)?;
            Ok(credentials.get(credential_id).cloned())
        })
    }

    fn list<'a>(
        &'a self,
        user_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<PasskeyCredential>>> + Send + 'a>> {
        Box::pin(async move {
            let credentials = self
                .credentials
                .read()
                .map_err(|_| AuthError::StoreUnavailable)?;
            let mut owned: Vec<PasskeyCredential> = credentials
                .values()
                .filter(|credential| credential.user_id == user_id)
                .cloned()
                .collect();
            owned.sort_by(|left, right| left.created_at.cmp(&right.created_at));
            Ok(owned)
        })
    }

    fn delete<'a>(
        &'a self,
        user_id: &'a str,
        credential_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut credentials = self
                .credentials
                .write()
                .map_err(|_| AuthError::StoreUnavailable)?;
            if credentials
                .get(credential_id)
                .is_some_and(|credential| credential.user_id == user_id)
            {
                credentials.remove(credential_id);
            }
            Ok(())
        })
    }

    fn update_counter<'a>(
        &'a self,
        credential_id: &'a str,
        counter: u32,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut credentials = self
                .credentials
                .write()
                .map_err(|_| AuthError::StoreUnavailable)?;
            let credential = credentials
                .get_mut(credential_id)
                .ok_or_else(|| AuthError::Passkey("credential not found".into()))?;
            credential.counter = counter;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A credential for `user_id` with the given id.
    fn credential(id: &str, user_id: &str) -> PasskeyCredential {
        PasskeyCredential {
            id: id.to_string(),
            user_id: user_id.to_string(),
            public_key: vec![4u8; 65],
            counter: 0,
            name: "Laptop".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    /// The in-memory store round-trips create/find/list/delete/update_counter.
    #[tokio::test]
    async fn memory_store_round_trip() {
        let store = MemoryPasskeyStore::default();
        assert!(store.find("c1").await.expect("find").is_none());

        store.create(credential("c1", "u1")).await.expect("create");
        store.create(credential("c2", "u2")).await.expect("create");
        assert_eq!(store.list("u1").await.expect("list").len(), 1);

        store.update_counter("c1", 7).await.expect("update_counter");
        assert_eq!(store.get_sync("c1").expect("seeded").counter, 7);

        // A user cannot delete another user's credential.
        store.delete("u2", "c1").await.expect("delete");
        assert!(store.find("c1").await.expect("find").is_some());
        store.delete("u1", "c1").await.expect("delete");
        assert!(store.find("c1").await.expect("find").is_none());
    }

    /// A duplicate credential id is rejected.
    #[tokio::test]
    async fn duplicate_credential_is_rejected() {
        let store = MemoryPasskeyStore::default();
        store.create(credential("c1", "u1")).await.expect("create");
        assert!(store.create(credential("c1", "u1")).await.is_err());
    }

    /// The deny-all store fails every operation closed.
    #[tokio::test]
    async fn deny_all_store_denies() {
        let store = DenyAllPasskeyStore;
        assert!(store.create(credential("c1", "u1")).await.is_err());
        assert!(store.find("c1").await.is_err());
        assert!(store.list("u1").await.is_err());
        assert!(store.delete("u1", "c1").await.is_err());
        assert!(store.update_counter("c1", 1).await.is_err());
    }
}
