//! Password-reset token store seam + cryptographically random token minting
//! (AUTH-013).
//!
//! This is the RustaSea analogue of Laravel's `password_reset_tokens` table and
//! the `PasswordBroker` token half. RustaSea has no database-backed token table
//! wired yet, so the persistence boundary is an explicit trait —
//! [`PasswordResetStore`] — exactly as user writes go through
//! [`crate::users::UserProvider`]. A database-backed implementation (over
//! `rustasea-orm`) resolves each call inside its boxed future; this crate stays
//! DB-agnostic and never depends on `sqlx`/`rustasea-orm`.
//!
//! # Hashed at rest (kit parity)
//!
//! The **plaintext token is never stored**. The caller mints a token with
//! [`generate_token`], hands the recipient the plaintext, and persists only an
//! Argon2 PHC **hash** of it ([`PasswordResetRecord::token_hash`]). A database
//! compromise therefore does not hand an attacker a working reset link — the
//! same posture Laravel takes (`Hash::make($token)`).
//!
//! # Fail-closed
//!
//! Every method returns [`crate::error::Result`], and the mutating methods have
//! **no default implementation**: an implementor must make an explicit choice
//! for every write. [`DenyAllResetStore`] is the fail-closed default (mirrors
//! [`crate::users::DenyAllProvider`]); a store that is un-wired or unavailable
//! returns `Err`, never a silent `Ok`.
//!
//! # Async convention
//!
//! Like [`crate::users::UserProvider`], each method returns a hand-rolled
//! `Pin<Box<dyn Future<..> + Send>>` rather than `async fn`, keeping the trait
//! object-safe (`&dyn PasswordResetStore`) and `Send`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::RwLock;

use argon2::password_hash::rand_core::{OsRng, RngCore};

use crate::error::{AuthError, Result};

/// Length of a generated reset token, in characters (kit parity: `Str::random(64)`).
pub const RESET_TOKEN_LEN: usize = 64;

/// Alphabet used for token generation: `A-Z a-z 0-9` (62 symbols).
const TOKEN_ALPHABET: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// Rejection threshold keeping the byte→symbol mapping uniform: `256 - (256 % 62)`
/// is `248`, so a byte in `0..248` maps onto exactly four copies of each symbol.
const TOKEN_BYTE_LIMIT: u8 = 248;

/// One persisted reset-token row (the `password_reset_tokens` shape).
///
/// Mirrors the migration template's columns: `email` is the primary key,
/// `token` is the stored (hashed) secret, and `created_at` is the UNIX-seconds
/// mint time the expiry window is measured from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordResetRecord {
    /// Account the token was minted for.
    pub email: String,
    /// Argon2 PHC hash of the plaintext token (never the token itself).
    pub token_hash: String,
    /// Mint time as UNIX seconds (expiry is `created_at + expire`).
    pub created_at: i64,
}

/// Async persistence contract for password-reset tokens.
///
/// Sufficient for the request/consume flow: `create` upserts the hashed token
/// for an email, `find` reads it back, and `delete` removes it on a successful
/// reset (single-use).
pub trait PasswordResetStore: Send + Sync {
    /// Upsert the hashed token for `email`, replacing any prior row.
    ///
    /// Laravel deletes any existing row before inserting, so one email holds at
    /// most one live token; a replacement implementation must match that.
    fn create<'a>(
        &'a self,
        email: &'a str,
        token_hash: &'a str,
        created_at: i64,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

    /// Resolve the stored token row for `email`.
    ///
    /// `Ok(None)` means no live token exists; an un-wired or unavailable store
    /// returns `Err`, never a silent `Ok(None)`.
    fn find<'a>(
        &'a self,
        email: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<PasswordResetRecord>>> + Send + 'a>>;

    /// Delete the token row for `email` (single-use invalidation).
    ///
    /// Deleting an absent row is not an error — the post-condition is "no token
    /// for this email".
    fn delete<'a>(
        &'a self,
        email: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;
}

/// Generate a cryptographically random reset token (64 chars, kit parity).
///
/// Uses the OS CSPRNG (`argon2`'s `rand_core` re-export, so no new dependency)
/// and rejection sampling so every symbol is equally likely — a biased token
/// would shrink the effective keyspace. The plaintext is returned to the caller
/// (to place in the reset link); only its hash is ever persisted.
pub fn generate_token() -> String {
    let mut rng = OsRng;
    let mut token = String::with_capacity(RESET_TOKEN_LEN);
    let mut buffer = [0u8; RESET_TOKEN_LEN];
    while token.len() < RESET_TOKEN_LEN {
        rng.fill_bytes(&mut buffer);
        for &byte in &buffer {
            if byte < TOKEN_BYTE_LIMIT {
                token.push(TOKEN_ALPHABET[(byte % 62) as usize] as char);
                if token.len() == RESET_TOKEN_LEN {
                    break;
                }
            }
        }
    }
    token
}

/// Fail-closed reset store: every operation is denied.
///
/// The default wiring posture until an application injects a real store
/// (ADR-0007 explicit wiring). Reads yield no token and writes are refused, so
/// an un-wired app fails closed rather than silently minting a token that can
/// never be persisted.
#[derive(Debug, Default)]
pub struct DenyAllResetStore;

impl DenyAllResetStore {
    /// Human-readable reason attached to every denial.
    const REASON: &'static str = "no password-reset store is wired; denying token access";
}

impl PasswordResetStore for DenyAllResetStore {
    fn create<'a>(
        &'a self,
        _email: &'a str,
        _token_hash: &'a str,
        _created_at: i64,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move { Err(AuthError::Disabled(Self::REASON.to_string())) })
    }

    fn find<'a>(
        &'a self,
        _email: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<PasswordResetRecord>>> + Send + 'a>> {
        Box::pin(async move { Err(AuthError::Disabled(Self::REASON.to_string())) })
    }

    fn delete<'a>(
        &'a self,
        _email: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move { Err(AuthError::Disabled(Self::REASON.to_string())) })
    }
}

/// In-memory [`PasswordResetStore`] for tests and local development.
///
/// **Test/dev only — not for production.** State lives in a process-local
/// `RwLock` map and is lost on restart: there is no durability and no
/// cross-process coordination. Use it to exercise the reset flow without a
/// database and inject a real store in production.
#[derive(Debug, Default)]
pub struct MemoryPasswordResetStore {
    /// Token rows keyed by email (one live token per email, kit parity).
    rows: RwLock<HashMap<String, PasswordResetRecord>>,
}

impl MemoryPasswordResetStore {
    /// Seed a token row directly (tests and local development).
    pub fn seed(&self, record: PasswordResetRecord) {
        if let Ok(mut rows) = self.rows.write() {
            rows.insert(record.email.clone(), record);
        }
    }

    /// Read the stored row for `email` (test probe).
    pub fn get(&self, email: &str) -> Option<PasswordResetRecord> {
        self.rows.read().ok()?.get(email).cloned()
    }
}

impl PasswordResetStore for MemoryPasswordResetStore {
    fn create<'a>(
        &'a self,
        email: &'a str,
        token_hash: &'a str,
        created_at: i64,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut rows = self.rows.write().map_err(|_| AuthError::StoreUnavailable)?;
            rows.insert(
                email.to_string(),
                PasswordResetRecord {
                    email: email.to_string(),
                    token_hash: token_hash.to_string(),
                    created_at,
                },
            );
            Ok(())
        })
    }

    fn find<'a>(
        &'a self,
        email: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<PasswordResetRecord>>> + Send + 'a>> {
        Box::pin(async move {
            let rows = self.rows.read().map_err(|_| AuthError::StoreUnavailable)?;
            Ok(rows.get(email).cloned())
        })
    }

    fn delete<'a>(
        &'a self,
        email: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut rows = self.rows.write().map_err(|_| AuthError::StoreUnavailable)?;
            rows.remove(email);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A generated token is the expected length and alphanumeric.
    #[test]
    fn generated_token_is_64_alphanumeric_chars() {
        let token = generate_token();
        assert_eq!(token.len(), RESET_TOKEN_LEN);
        assert!(token.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    /// Two generated tokens differ (the CSPRNG is not a constant).
    #[test]
    fn generated_tokens_are_distinct() {
        assert_ne!(generate_token(), generate_token());
    }

    /// The in-memory store round-trips create → find → delete.
    #[tokio::test]
    async fn memory_store_round_trips() {
        let store = MemoryPasswordResetStore::default();
        store
            .create("ada@example.com", "hash-1", 1_000)
            .await
            .expect("create succeeds");
        let record = store
            .find("ada@example.com")
            .await
            .expect("find succeeds")
            .expect("a record exists");
        assert_eq!(record.token_hash, "hash-1");
        assert_eq!(record.created_at, 1_000);
        store
            .delete("ada@example.com")
            .await
            .expect("delete succeeds");
        assert!(store.find("ada@example.com").await.expect("find").is_none());
    }

    /// `create` replaces an existing row (one live token per email).
    #[tokio::test]
    async fn memory_store_create_replaces() {
        let store = MemoryPasswordResetStore::default();
        store.create("ada@example.com", "hash-1", 1).await.unwrap();
        store.create("ada@example.com", "hash-2", 2).await.unwrap();
        assert_eq!(
            store
                .get("ada@example.com")
                .expect("a row exists")
                .token_hash,
            "hash-2"
        );
    }

    /// The deny-all store fails every operation closed.
    #[tokio::test]
    async fn deny_all_store_fails_closed() {
        let store = DenyAllResetStore;
        assert!(store.create("a@b.c", "h", 0).await.is_err());
        assert!(store.find("a@b.c").await.is_err());
        assert!(store.delete("a@b.c").await.is_err());
    }
}
