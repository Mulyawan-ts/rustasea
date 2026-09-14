//! Two-factor authentication (AUTH-016) — TOTP lifecycle + recovery codes.
//!
//! This module is the RustaSea analogue of Laravel Fortify's two-factor
//! feature: it mints a TOTP secret with an `otpauth://` provisioning URI,
//! verifies six-digit codes with a configurable clock-skew window, and issues
//! single-use recovery codes. The persistence boundary is the async
//! [`TwoFactorStore`] seam and the at-rest boundary is [`SecretCipher`]
//! (ChaCha20-Poly1305 keyed from `app.key`).
//!
//! # What is stored
//!
//! * the **secret** is sealed with [`SecretCipher`] — never stored or logged in
//!   the clear;
//! * **recovery codes** are stored as Argon2 PHC hashes and consumed by
//!   removing the matched hash, so each code works exactly once.
//!
//! # Fail closed
//!
//! A blank `app.key` cannot build a cipher, a malformed/tampered envelope fails
//! decryption, and an un-wired store denies every call — so a misconfigured app
//! rejects two-factor operations rather than degrading to a bypassable factor.
//!
//! # Time
//!
//! Verification takes `now_secs` from the caller (the service passes the wall
//! clock), keeping the RFC 6238 math deterministic in tests.

pub mod cipher;
pub mod store;
pub mod totp;

pub use cipher::SecretCipher;
pub use store::{DenyAllTwoFactorStore, MemoryTwoFactorStore, TwoFactorRecord, TwoFactorStore};
pub use totp::{
    code_at, generate_secret, otpauth_uri, verify_code, DEFAULT_WINDOW, DIGITS, SECRET_BYTES,
    STEP_SECS,
};

use std::sync::Arc;

use argon2::password_hash::rand_core::{OsRng, RngCore};
use chrono::Utc;

use crate::error::{AuthError, Result};
use crate::verify::{Argon2Verifier, PasswordVerifier};

/// Number of recovery codes minted per set (kit parity).
pub const RECOVERY_CODE_COUNT: usize = 10;

/// Characters per recovery-code group (`abcde-12345` = two groups of five).
const RECOVERY_GROUP_LEN: usize = 5;

/// Letters in the recovery-code alphabet.
const RECOVERY_LETTERS: u8 = 26;

/// Digits in the recovery-code alphabet.
const RECOVERY_DIGITS: u8 = 10;

/// Result of enabling two-factor authentication.
///
/// The secret and recovery codes are returned **once** for display; only their
/// sealed / hashed forms are persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoFactorEnrollment {
    /// Plaintext base32 secret to scan/enter once.
    pub secret: String,
    /// `otpauth://` provisioning URI for the authenticator app.
    pub otpauth_uri: String,
    /// Plaintext recovery codes (shown once; only hashes are stored).
    pub recovery_codes: Vec<String>,
}

/// Orchestrates the TOTP lifecycle over a [`TwoFactorStore`].
pub struct TwoFactorService {
    /// Persistence seam for two-factor state.
    store: Arc<dyn TwoFactorStore>,
    /// AEAD used to seal the secret at rest.
    cipher: SecretCipher,
    /// Issuer name shown in the authenticator app.
    issuer: String,
    /// Steps accepted either side of "now" when verifying a code.
    window: u32,
}

impl std::fmt::Debug for TwoFactorService {
    /// Manual debug — the cipher and store are rendered opaquely.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TwoFactorService")
            .field("issuer", &self.issuer)
            .field("window", &self.window)
            .finish_non_exhaustive()
    }
}

impl TwoFactorService {
    /// Build a service over `store`/`cipher` with an explicit issuer + window.
    pub fn new(
        store: Arc<dyn TwoFactorStore>,
        cipher: SecretCipher,
        issuer: impl Into<String>,
        window: u32,
    ) -> Self {
        Self {
            store,
            cipher,
            issuer: issuer.into(),
            window,
        }
    }

    /// Whether `user_id` has a confirmed two-factor secret.
    ///
    /// Reads only the confirmation timestamp, so it succeeds without the
    /// cipher — the login interception needs this before any secret handling.
    pub async fn is_confirmed(&self, user_id: &str) -> Result<bool> {
        Ok(self
            .store
            .get(user_id)
            .await?
            .is_some_and(|record| record.is_confirmed()))
    }

    /// Generate and persist a fresh (unconfirmed) secret + recovery codes.
    ///
    /// The secret is sealed before it is stored; the returned
    /// [`TwoFactorEnrollment`] carries the plaintext values for the one-time
    /// display. A prior record for the user is replaced.
    pub async fn enable(&self, user_id: &str, account: &str) -> Result<TwoFactorEnrollment> {
        let secret = generate_secret();
        let otpauth_uri = otpauth_uri(&self.issuer, account, &secret);
        let recovery_codes = generate_recovery_codes();
        let sealed = self.cipher.encrypt(&secret)?;
        let hashes = hash_recovery_codes(&recovery_codes)?;
        self.store
            .put(TwoFactorRecord {
                user_id: user_id.to_string(),
                secret: Some(sealed),
                recovery_codes: hashes,
                confirmed_at: None,
            })
            .await?;
        Ok(TwoFactorEnrollment {
            secret,
            otpauth_uri,
            recovery_codes,
        })
    }

    /// Confirm the pending secret with a valid TOTP code.
    ///
    /// Sets `confirmed_at` only when the code verifies; an absent/unconfirmed
    /// record or an invalid code is [`AuthError::InvalidTwoFactorCode`].
    pub async fn confirm(&self, user_id: &str, code: &str) -> Result<()> {
        let Some(mut record) = self.store.get(user_id).await? else {
            return Err(AuthError::InvalidTwoFactorCode);
        };
        let Some(sealed) = record.secret.as_deref() else {
            return Err(AuthError::InvalidTwoFactorCode);
        };
        let secret = self.cipher.decrypt(sealed)?;
        if !verify_code(&secret, code, self.window, now_secs()) {
            return Err(AuthError::InvalidTwoFactorCode);
        }
        record.confirmed_at = Some(Utc::now().to_rfc3339());
        self.store.put(record).await
    }

    /// Disable two-factor authentication by deleting the user's record.
    pub async fn disable(&self, user_id: &str) -> Result<()> {
        self.store.delete(user_id).await
    }

    /// Verify a TOTP code during the login challenge.
    ///
    /// Requires a confirmed record; an unconfirmed/absent record or a code
    /// outside the window is [`AuthError::InvalidTwoFactorCode`]. A missing
    /// cipher key or a tampered envelope is a typed [`AuthError::TwoFactor`].
    pub async fn verify_challenge(&self, user_id: &str, code: &str) -> Result<()> {
        let Some(record) = self.store.get(user_id).await? else {
            return Err(AuthError::InvalidTwoFactorCode);
        };
        if !record.is_confirmed() {
            return Err(AuthError::InvalidTwoFactorCode);
        }
        let Some(sealed) = record.secret.as_deref() else {
            return Err(AuthError::InvalidTwoFactorCode);
        };
        let secret = self.cipher.decrypt(sealed)?;
        if verify_code(&secret, code, self.window, now_secs()) {
            Ok(())
        } else {
            Err(AuthError::InvalidTwoFactorCode)
        }
    }

    /// Consume a single-use recovery code, removing its stored hash.
    ///
    /// Returns [`AuthError::InvalidTwoFactorCode`] when no stored hash matches
    /// (including a code that was already consumed).
    pub async fn consume_recovery_code(&self, user_id: &str, code: &str) -> Result<()> {
        let Some(mut record) = self.store.get(user_id).await? else {
            return Err(AuthError::InvalidTwoFactorCode);
        };
        let verifier = Argon2Verifier::new();
        let position = record
            .recovery_codes
            .iter()
            .position(|hash| verifier.verify(hash, code));
        match position {
            Some(index) => {
                record.recovery_codes.remove(index);
                self.store.put(record).await
            }
            None => Err(AuthError::InvalidTwoFactorCode),
        }
    }

    /// Mint a fresh recovery-code set, replacing the stored hashes.
    ///
    /// Returns the plaintext codes for display. Requires an existing secret
    /// (two-factor enabled); otherwise [`AuthError::TwoFactorRequired`].
    pub async fn regenerate_recovery_codes(&self, user_id: &str) -> Result<Vec<String>> {
        let Some(mut record) = self.store.get(user_id).await? else {
            return Err(AuthError::TwoFactorRequired);
        };
        if record.secret.is_none() {
            return Err(AuthError::TwoFactorRequired);
        }
        let codes = generate_recovery_codes();
        record.recovery_codes = hash_recovery_codes(&codes)?;
        self.store.put(record).await?;
        Ok(codes)
    }
}

/// Current UNIX time in seconds (clamped at zero).
fn now_secs() -> u64 {
    u64::try_from(Utc::now().timestamp()).unwrap_or(0)
}

/// Generate a fresh set of unique recovery codes.
fn generate_recovery_codes() -> Vec<String> {
    let mut codes = Vec::with_capacity(RECOVERY_CODE_COUNT);
    while codes.len() < RECOVERY_CODE_COUNT {
        let code = random_recovery_code();
        if !codes.contains(&code) {
            codes.push(code);
        }
    }
    codes
}

/// Mint one `abcde-12345` recovery code from the OS CSPRNG.
///
/// The modulo reduction over `a-z`/`0-9` is negligibly biased and the code is
/// one of ten; combined entropy (`26^5 * 10^5` ≈ 2^40) is far beyond guessing.
fn random_recovery_code() -> String {
    let mut bytes = [0u8; RECOVERY_GROUP_LEN * 2];
    OsRng.fill_bytes(&mut bytes);
    let mut code = String::with_capacity(RECOVERY_GROUP_LEN * 2 + 1);
    for (index, byte) in bytes.iter().enumerate() {
        if index == RECOVERY_GROUP_LEN {
            code.push('-');
        }
        if index < RECOVERY_GROUP_LEN {
            code.push((b'a' + (byte % RECOVERY_LETTERS)) as char);
        } else {
            code.push((b'0' + (byte % RECOVERY_DIGITS)) as char);
        }
    }
    code
}

/// Hash every recovery code with Argon2 (PHC strings) for storage.
fn hash_recovery_codes(codes: &[String]) -> Result<Vec<String>> {
    let verifier = Argon2Verifier::new();
    codes.iter().map(|code| verifier.hash(code)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a service over an in-memory store with a fixed test key.
    fn service() -> (TwoFactorService, Arc<MemoryTwoFactorStore>) {
        let store = Arc::new(MemoryTwoFactorStore::default());
        let cipher = SecretCipher::from_bytes([9u8; 32]);
        (
            TwoFactorService::new(store.clone(), cipher, "RustaSea", DEFAULT_WINDOW),
            store,
        )
    }

    /// Enable seals the secret, then a valid code confirms it.
    #[tokio::test]
    async fn enable_then_confirm() {
        let (service, store) = service();
        let enrollment = service
            .enable("user-1", "ada@example.com")
            .await
            .expect("enable");
        assert_eq!(enrollment.recovery_codes.len(), RECOVERY_CODE_COUNT);
        assert!(enrollment.otpauth_uri.starts_with("otpauth://totp/"));

        // The secret is sealed at rest, never stored in the clear.
        let record = store.get_sync("user-1").expect("record");
        assert_ne!(record.secret.as_deref(), Some(enrollment.secret.as_str()));
        assert!(!record.is_confirmed());

        let code = code_at(&enrollment.secret, now_secs()).expect("current code");
        service.confirm("user-1", &code).await.expect("confirm");
        assert!(service.is_confirmed("user-1").await.expect("is_confirmed"));
    }

    /// An invalid code is rejected and never confirms.
    #[tokio::test]
    async fn confirm_rejects_invalid_code() {
        let (service, _store) = service();
        service
            .enable("user-1", "ada@example.com")
            .await
            .expect("enable");
        assert_eq!(
            service.confirm("user-1", "000000").await,
            Err(AuthError::InvalidTwoFactorCode)
        );
        assert!(!service.is_confirmed("user-1").await.expect("is_confirmed"));
    }

    /// A confirmed secret verifies a challenge code; disabling wipes it.
    #[tokio::test]
    async fn challenge_verifies_and_disable_wipes() {
        let (service, _store) = service();
        let enrollment = service
            .enable("user-1", "ada@example.com")
            .await
            .expect("enable");
        let code = code_at(&enrollment.secret, now_secs()).expect("code");
        service.confirm("user-1", &code).await.expect("confirm");
        service
            .verify_challenge("user-1", &code)
            .await
            .expect("challenge");
        assert_eq!(
            service.verify_challenge("user-1", "999999").await,
            Err(AuthError::InvalidTwoFactorCode)
        );

        service.disable("user-1").await.expect("disable");
        assert!(!service.is_confirmed("user-1").await.expect("is_confirmed"));
        assert_eq!(
            service.verify_challenge("user-1", &code).await,
            Err(AuthError::InvalidTwoFactorCode)
        );
    }

    /// A recovery code is consumed exactly once.
    #[tokio::test]
    async fn recovery_code_is_single_use() {
        let (service, _store) = service();
        let enrollment = service
            .enable("user-1", "ada@example.com")
            .await
            .expect("enable");
        let code = enrollment.recovery_codes[0].clone();
        service
            .consume_recovery_code("user-1", &code)
            .await
            .expect("first use");
        assert_eq!(
            service.consume_recovery_code("user-1", &code).await,
            Err(AuthError::InvalidTwoFactorCode)
        );
    }

    /// Regenerating replaces the set; the old codes no longer work.
    #[tokio::test]
    async fn regenerate_replaces_codes() {
        let (service, _store) = service();
        let enrollment = service
            .enable("user-1", "ada@example.com")
            .await
            .expect("enable");
        let old = enrollment.recovery_codes[0].clone();
        let fresh = service
            .regenerate_recovery_codes("user-1")
            .await
            .expect("regenerate");
        assert_eq!(fresh.len(), RECOVERY_CODE_COUNT);
        assert_eq!(
            service.consume_recovery_code("user-1", &old).await,
            Err(AuthError::InvalidTwoFactorCode)
        );
    }
}
