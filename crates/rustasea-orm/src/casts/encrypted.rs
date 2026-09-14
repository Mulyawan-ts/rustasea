//! The `encrypted` attribute cast — authenticated encryption at rest.
//!
//! Plaintext is sealed with ChaCha20-Poly1305 under a 256-bit key derived from
//! the application key (`SHA-256(app_key)`), mirroring the framework's
//! two-factor secret cipher. The envelope is `hex(nonce || ciphertext || tag)`,
//! so a tampered or foreign-key ciphertext fails authentication and surfaces as
//! [`OrmError::CastError`](crate::error::OrmError::CastError) rather than
//! yielding attacker-controlled plaintext.

use crate::casts::{cast_error, CastsAttributes};
use crate::error::Result;
use crate::types::Value;
use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::{Aead, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use sha2::{Digest, Sha256};

/// Nonce length for ChaCha20-Poly1305 (96 bits).
const NONCE_LEN: usize = 12;

/// Environment variables consulted by [`EncryptedCast::from_env`], in priority order.
const APP_KEY_ENV: [&str; 2] = ["RUSTASEA_APP_KEY", "APP_KEY"];

/// Casts a column to/from an encrypted text envelope.
///
/// The key is resolved once at construction. When no key is configured the cast
/// fails closed on use with a typed cast error, so an unconfigured app can never
/// write plaintext or a default-key ciphertext.
pub struct EncryptedCast {
    /// 256-bit ChaCha20-Poly1305 key, or `None` when unconfigured.
    key: Option<[u8; 32]>,
}

impl std::fmt::Debug for EncryptedCast {
    /// Manual debug — the key is never rendered.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EncryptedCast")
            .field("key", &"[redacted]")
            .finish()
    }
}

impl Default for EncryptedCast {
    /// Resolve the key from the environment.
    fn default() -> Self {
        Self::from_env()
    }
}

impl EncryptedCast {
    /// Derive the key from an application key of any non-blank length.
    ///
    /// A blank key yields an unconfigured cast (fails closed on use).
    pub fn from_app_key(app_key: &str) -> Self {
        let trimmed = app_key.trim();
        if trimmed.is_empty() {
            return Self { key: None };
        }
        let digest = Sha256::digest(trimmed.as_bytes());
        let mut key = [0u8; 32];
        key.copy_from_slice(&digest);
        Self { key: Some(key) }
    }

    /// Build from raw 32-byte key material (explicit wiring/tests).
    pub fn from_bytes(key: [u8; 32]) -> Self {
        Self { key: Some(key) }
    }

    /// Resolve the key from `RUSTASEA_APP_KEY`, then `APP_KEY`.
    pub fn from_env() -> Self {
        for variable in APP_KEY_ENV {
            if let Ok(value) = std::env::var(variable) {
                if !value.trim().is_empty() {
                    return Self::from_app_key(&value);
                }
            }
        }
        Self { key: None }
    }

    /// Borrow the configured key or fail closed with a typed cast error.
    fn key(&self, column: &str) -> Result<&[u8; 32]> {
        self.key.as_ref().ok_or_else(|| {
            cast_error(
                column,
                "encrypted cast requires a 32-byte key (set RUSTASEA_APP_KEY or APP_KEY)",
            )
        })
    }
}

impl CastsAttributes<String> for EncryptedCast {
    /// Open a hex envelope into its plaintext; `NULL` hydrates to an empty string.
    fn get(&self, column: &str, value: &Value) -> Result<String> {
        match value {
            Value::Null => Ok(String::new()),
            Value::Text(envelope) => {
                let key = self.key(column)?;
                let bytes = hex::decode(envelope)
                    .map_err(|_| cast_error(column, "encrypted value is not valid hex"))?;
                if bytes.len() <= NONCE_LEN {
                    return Err(cast_error(column, "encrypted envelope is truncated"));
                }
                let (nonce, ciphertext) = bytes.split_at(NONCE_LEN);
                let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
                let plaintext = cipher
                    .decrypt(Nonce::from_slice(nonce), ciphertext)
                    .map_err(|_| cast_error(column, "encrypted value failed authentication"))?;
                String::from_utf8(plaintext)
                    .map_err(|_| cast_error(column, "encrypted value is not valid UTF-8"))
            }
            other => Err(cast_error(
                column,
                format!("encrypted cast expects text, got {other:?}"),
            )),
        }
    }

    /// Seal plaintext into a hex envelope; an empty string persists as `NULL`.
    fn set(&self, column: &str, value: &String) -> Result<Value> {
        if value.is_empty() {
            return Ok(Value::Null);
        }
        let key = self.key(column)?;
        let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
        let mut nonce_bytes = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce_bytes);
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce_bytes), value.as_bytes())
            .map_err(|_| cast_error(column, "encryption failed"))?;
        let mut envelope = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        envelope.extend_from_slice(&nonce_bytes);
        envelope.extend_from_slice(&ciphertext);
        Ok(Value::Text(hex::encode(envelope)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies the encrypted cast round-trips and fails closed without a key.
    #[test]
    fn round_trip_and_fail_closed() {
        let cast = EncryptedCast::from_bytes([7u8; 32]);
        let sealed = cast.set("secret", &"top-secret".to_string()).unwrap();
        assert!(matches!(sealed, Value::Text(_)));
        let opened = cast.get("secret", &sealed).unwrap();
        assert_eq!(opened, "top-secret");

        let unconfigured = EncryptedCast::from_app_key("   ");
        assert!(unconfigured.get("secret", &sealed).is_err());
        assert!(unconfigured.set("secret", &"x".to_string()).is_err());
    }

    /// Verifies a foreign key and tampered envelope both fail closed.
    #[test]
    fn foreign_key_and_tamper_fail() {
        let cast = EncryptedCast::from_bytes([1u8; 32]);
        let sealed = cast.set("secret", &"value".to_string()).unwrap();
        let Value::Text(envelope) = sealed else {
            panic!("expected a text envelope");
        };
        let foreign = EncryptedCast::from_bytes([2u8; 32]);
        assert!(foreign
            .get("secret", &Value::Text(envelope.clone()))
            .is_err());

        let mut bytes = hex::decode(&envelope).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        assert!(cast
            .get("secret", &Value::Text(hex::encode(bytes)))
            .is_err());
    }
}
