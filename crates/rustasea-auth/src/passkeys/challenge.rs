//! WebAuthn challenge tracking seam + in-memory / fail-closed implementations.
//!
//! A WebAuthn ceremony is a two-round exchange: the server mints a random
//! challenge and the client returns it inside the signed `clientDataJSON`. The
//! challenge must be **single-use** (a replay must not verify) and **bound** to
//! the ceremony's context, so the HTTP layer stores it under a key that ties it
//! to the session (login) or the authenticated account (registration).
//!
//! [`ChallengeStore::take`] is the security-critical operation: it consumes the
//! challenge atomically, so a captured response cannot be replayed even within
//! its timeout window.
//!
//! # Async convention
//!
//! Like [`crate::users::UserProvider`], each method returns a hand-rolled
//! `Pin<Box<dyn Future<..> + Send>>`, keeping the trait object-safe and `Send`.
//!
//! # Fail closed
//!
//! [`DenyAllChallengeStore`] denies every operation, so an un-wired app rejects
//! every ceremony rather than accepting an un-tracked challenge.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::RwLock;

use crate::error::{AuthError, Result};

/// Async, one-time challenge-tracking contract for WebAuthn ceremonies.
pub trait ChallengeStore: Send + Sync {
    /// Store `challenge` under `key`, replacing any pending value.
    fn put<'a>(
        &'a self,
        key: &'a str,
        challenge: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

    /// Consume and return the challenge stored under `key`.
    ///
    /// The challenge is removed as part of the call, so a second `take` for the
    /// same key yields `None` — the single-use guarantee. `Ok(None)` means no
    /// pending challenge; an un-wired store returns `Err`.
    fn take<'a>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>>> + Send + 'a>>;
}

/// Fail-closed challenge store: every operation is denied.
#[derive(Debug, Default)]
pub struct DenyAllChallengeStore;

impl DenyAllChallengeStore {
    /// Human-readable reason attached to every denial.
    const REASON: &'static str = "no challenge store is wired; denying access";
}

impl ChallengeStore for DenyAllChallengeStore {
    fn put<'a>(
        &'a self,
        _key: &'a str,
        _challenge: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move { Err(AuthError::Disabled(Self::REASON.to_string())) })
    }

    fn take<'a>(
        &'a self,
        _key: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>>> + Send + 'a>> {
        Box::pin(async move { Err(AuthError::Disabled(Self::REASON.to_string())) })
    }
}

/// In-memory [`ChallengeStore`] for tests and local development.
///
/// **Test/dev only — not for production.** State lives in a process-local
/// `RwLock` map and is lost on restart; inject a real store in production.
#[derive(Debug, Default)]
pub struct MemoryChallengeStore {
    /// Pending challenges keyed by ceremony key.
    challenges: RwLock<HashMap<String, String>>,
}

impl MemoryChallengeStore {
    /// Read a pending challenge synchronously (test assertions).
    pub fn get_sync(&self, key: &str) -> Option<String> {
        self.challenges.read().ok()?.get(key).cloned()
    }
}

impl ChallengeStore for MemoryChallengeStore {
    fn put<'a>(
        &'a self,
        key: &'a str,
        challenge: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut challenges = self
                .challenges
                .write()
                .map_err(|_| AuthError::StoreUnavailable)?;
            challenges.insert(key.to_string(), challenge.to_string());
            Ok(())
        })
    }

    fn take<'a>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>>> + Send + 'a>> {
        Box::pin(async move {
            let mut challenges = self
                .challenges
                .write()
                .map_err(|_| AuthError::StoreUnavailable)?;
            Ok(challenges.remove(key))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stored challenge is returned exactly once.
    #[tokio::test]
    async fn challenge_is_single_use() {
        let store = MemoryChallengeStore::default();
        store.put("k", "abc").await.expect("put");
        assert_eq!(store.take("k").await.expect("take").as_deref(), Some("abc"));
        assert!(store.take("k").await.expect("second take").is_none());
    }

    /// The deny-all store fails every operation closed.
    #[tokio::test]
    async fn deny_all_store_denies() {
        let store = DenyAllChallengeStore;
        assert!(store.put("k", "abc").await.is_err());
        assert!(store.take("k").await.is_err());
    }
}
