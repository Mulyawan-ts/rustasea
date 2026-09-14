//! Authenticated encryption for two-factor secrets at rest.
//!
//! The at-rest boundary is ChaCha20-Poly1305 (an AEAD): a fresh 96-bit nonce is
//! generated per encryption and the 128-bit Poly1305 tag authenticates the
//! ciphertext, so a tampered envelope fails decryption rather than yielding
//! attacker-controlled plaintext. The 256-bit key is derived from `app.key`
//! with SHA-256, so no separate two-factor key needs provisioning.
//!
//! # Envelope
//!
//! `hex(nonce || ciphertext || tag)` — hex keeps the format text-safe and
//! reuses the workspace `hex` crate, so no base64 dependency is required.
//!
//! # Fail closed
//!
//! [`SecretCipher::from_app_key`] rejects a blank key, so an unconfigured app
//! cannot seal a secret with a default key; every decrypt failure is a typed
//! [`AuthError::TwoFactor`] rather than a silent `Ok`.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use sha2::{Digest, Sha256};

use crate::error::AuthError;

/// Nonce length for ChaCha20-Poly1305 (96 bits).
const NONCE_LEN: usize = 12;

/// Keyed AEAD used to seal two-factor material at rest.
#[derive(Clone)]
pub struct SecretCipher {
    /// 256-bit ChaCha20-Poly1305 key.
    key: [u8; 32],
}

impl std::fmt::Debug for SecretCipher {
    /// Manual debug — the key is never rendered.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretCipher")
            .field("key", &"[redacted]")
            .finish()
    }
}

impl SecretCipher {
    /// Derive a cipher from the application key; fail closed on a blank key.
    ///
    /// The 32-byte key is `SHA-256(app_key)`, so any configured key length
    /// yields a full-size AEAD key deterministically.
    pub fn from_app_key(app_key: &str) -> Result<Self, AuthError> {
        let trimmed = app_key.trim();
        if trimmed.is_empty() {
            return Err(AuthError::TwoFactor(
                "app.key is not configured".to_string(),
            ));
        }
        let digest = Sha256::digest(trimmed.as_bytes());
        let mut key = [0u8; 32];
        key.copy_from_slice(&digest);
        Ok(Self { key })
    }

    /// Build a cipher from raw 32-byte key material (explicit wiring/tests).
    pub fn from_bytes(key: [u8; 32]) -> Self {
        Self { key }
    }

    /// Seal `plaintext` into a hex envelope (`nonce || ciphertext || tag`).
    pub fn encrypt(&self, plaintext: &str) -> Result<String, AuthError> {
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.key));
        let mut nonce_bytes = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|_| AuthError::TwoFactor("secret encryption failed".to_string()))?;
        let mut envelope = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        envelope.extend_from_slice(&nonce_bytes);
        envelope.extend_from_slice(&ciphertext);
        Ok(hex::encode(envelope))
    }

    /// Open a hex envelope produced by [`SecretCipher::encrypt`].
    ///
    /// A malformed, truncated, or tampered envelope is a typed
    /// [`AuthError::TwoFactor`] — never a partial plaintext.
    pub fn decrypt(&self, envelope: &str) -> Result<String, AuthError> {
        let malformed = || AuthError::TwoFactor("malformed secret envelope".to_string());
        let bytes = hex::decode(envelope).map_err(|_| malformed())?;
        if bytes.len() <= NONCE_LEN {
            return Err(malformed());
        }
        let (nonce_bytes, ciphertext) = bytes.split_at(NONCE_LEN);
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.key));
        let plaintext = cipher
            .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
            .map_err(|_| AuthError::TwoFactor("secret decryption failed".to_string()))?;
        String::from_utf8(plaintext)
            .map_err(|_| AuthError::TwoFactor("secret is not valid UTF-8".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cipher round-trips plaintext and rejects a blank app key.
    #[test]
    fn round_trip_and_blank_key_rejection() {
        let cipher = SecretCipher::from_app_key("base64:test-application-key")
            .expect("a non-blank key builds a cipher");
        let sealed = cipher.encrypt("JBSWY3DPEHPK3PXP").expect("seal");
        assert_ne!(sealed, "JBSWY3DPEHPK3PXP", "the envelope is not plaintext");
        assert_eq!(cipher.decrypt(&sealed).as_deref(), Ok("JBSWY3DPEHPK3PXP"));

        assert!(SecretCipher::from_app_key("   ").is_err());
    }

    /// Two encryptions of the same plaintext differ (fresh nonce) yet decrypt.
    #[test]
    fn fresh_nonce_per_encryption() {
        let cipher = SecretCipher::from_bytes([7u8; 32]);
        let first = cipher.encrypt("same").expect("first");
        let second = cipher.encrypt("same").expect("second");
        assert_ne!(first, second);
        assert_eq!(cipher.decrypt(&first).as_deref(), Ok("same"));
        assert_eq!(cipher.decrypt(&second).as_deref(), Ok("same"));
    }

    /// A tampered envelope and a foreign key both fail closed.
    #[test]
    fn tamper_and_foreign_key_fail_closed() {
        let cipher = SecretCipher::from_bytes([1u8; 32]);
        let sealed = cipher.encrypt("secret").expect("seal");
        let mut bytes = hex::decode(&sealed).expect("hex");
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        assert!(cipher.decrypt(&hex::encode(bytes)).is_err());

        let foreign = SecretCipher::from_bytes([2u8; 32]);
        assert!(foreign.decrypt(&sealed).is_err());
        assert!(cipher.decrypt("not-hex").is_err());
    }
}
