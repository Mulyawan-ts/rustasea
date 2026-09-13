//! Signed-URL infrastructure — HMAC-SHA256 + expiry + tamper detection.
//!
//! The RustaSea analogue of Laravel's `URL::temporarySignedRoute` / `signed`
//! middleware: a stateless, tamper-evident URL carrying its own expiry,
//! verifiable without server-side storage. It backs the email-verification and
//! password-reset links (AUTH-013/AUTH-014); the router re-exports
//! [`SignedUrlSigner`], avoiding a dependency cycle.
//!
//! # Canonical payload (relied on by AUTH-013/AUTH-014)
//!
//! The HMAC is over a UTF-8 string, one `\n`-terminated field per line — the
//! percent-encoded path, the decimal expiry, then each sorted `key=value` pair
//! — so framing is unambiguous even when a path or parameter contains `&`, `=`,
//! or `\n`. Sorting makes the signature **order-independent**; components use
//! the RFC 3986 unreserved set (`A-Z a-z 0-9 - . _ ~`), so `%`/`\n` cannot be
//! smuggled in; the digest is HMAC-SHA256, hex-encoded. [`SignedUrlSigner::sign`]
//! returns the **complete query fragment** (sorted `extra`, `expires=<unix>`,
//! `signature=<hex>`); feed it back to [`SignedUrlSigner::verify`]. The reserved
//! names `expires` and `signature` must not appear in `extra`. A missing or
//! blank `AppConfig::key` yields [`SignedUrlError::MissingKey`] — never a
//! silent default — and comparison uses [`subtle::ConstantTimeEq`] so it never
//! short-circuits. See [`SignedUrlSigner::from_app_config`] for the Laravel
//! `base64:` `APP_KEY` parity behaviour.
use hmac::{Hmac, Mac};
use rustasea_foundation::AppConfig;
use sha2::Sha256;
use std::sync::Arc;
use subtle::ConstantTimeEq;

/// HMAC-SHA256 instantiation used for every signature.
type HmacSha256 = Hmac<Sha256>;

/// Query parameter carrying the absolute expiry (UNIX seconds).
const EXPIRES_PARAM: &str = "expires";

/// Query parameter carrying the hex-encoded signature.
const SIGNATURE_PARAM: &str = "signature";

/// Typed failures for signed-URL operations; callers (AUTH-013/AUTH-014) map
/// them onto HTTP responses (`403` for a bad or expired signature).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SignedUrlError {
    /// The signing key is missing or blank (the fail-closed guarantee).
    #[error("signed URL signing key is missing or empty")]
    MissingKey,
    /// The query string carries no `signature` parameter.
    #[error("signed URL is missing its `signature` parameter")]
    MissingSignature,
    /// A signature is present but does not match the expected HMAC.
    #[error("signed URL signature is invalid")]
    InvalidSignature,
    /// The signature matched but the link's expiry is in the past.
    #[error("signed URL expired at {expired_at} (now {now})")]
    Expired {
        /// The absolute expiry encoded in the URL (UNIX seconds).
        expired_at: i64,
        /// The verification time (UNIX seconds).
        now: i64,
    },
    /// An unparsable query, or a `base64:` `APP_KEY` payload that is invalid.
    #[error("malformed signed URL: {detail}")]
    Malformed {
        /// Human-readable explanation of the parse failure.
        detail: String,
    },
}

impl SignedUrlError {
    /// Stable, machine-readable code (parity with [`crate::AuthError::code`]).
    pub fn code(&self) -> &'static str {
        match self {
            SignedUrlError::MissingKey => "MissingKey",
            SignedUrlError::MissingSignature => "MissingSignature",
            SignedUrlError::InvalidSignature => "InvalidSignature",
            SignedUrlError::Expired { .. } => "Expired",
            SignedUrlError::Malformed { .. } => "Malformed",
        }
    }
}

/// A clock returning the current UNIX time in whole seconds.
type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Signs and verifies tamper-evident, expiring URLs. Build with
/// [`SignedUrlSigner::from_app_config`] (production) or
/// [`SignedUrlSigner::new`] (tests); override the clock with
/// [`SignedUrlSigner::with_clock`] for deterministic tests.
pub struct SignedUrlSigner {
    /// Raw signing key bytes; an empty key makes the signer fail closed.
    key: Vec<u8>,
    /// Clock source; the system clock unless overridden.
    clock: Clock,
}

impl std::fmt::Debug for SignedUrlSigner {
    /// Redacts the key, printing only its length.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedUrlSigner")
            .field("key_len", &self.key.len())
            .finish_non_exhaustive()
    }
}

impl SignedUrlSigner {
    /// Build a signer from raw key bytes using the system clock. The key is
    /// taken verbatim; an **empty** key yields an inert signer (a `MissingKey`
    /// on verify/sign_expiring). Prefer [`SignedUrlSigner::from_app_config`].
    pub fn new(key: impl Into<Vec<u8>>) -> Self {
        Self {
            key: key.into(),
            clock: Arc::new(|| chrono::Utc::now().timestamp()),
        }
    }

    /// Build a signer from [`AppConfig`], failing closed when the key is
    /// missing or blank.
    ///
    /// # Laravel `APP_KEY` parity
    ///
    /// Laravel's `APP_KEY` is conventionally `base64:<base64-encoded-bytes>`.
    /// A key with that prefix is base64-decoded and the **decoded bytes** become
    /// the HMAC key; a key without it is used verbatim as raw string bytes
    /// (backward compatible). The remainder accepts the standard and URL-safe
    /// alphabets with optional `=` padding. An invalid payload is
    /// [`SignedUrlError::Malformed`] — never a silent fallback to the raw string.
    ///
    /// # Errors
    ///
    /// [`SignedUrlError::MissingKey`] when `config.key` is `None` or blank;
    /// [`SignedUrlError::Malformed`] when a `base64:` payload is invalid.
    pub fn from_app_config(config: &AppConfig) -> Result<Self, SignedUrlError> {
        match config.key.as_deref() {
            Some(key) if !key.trim().is_empty() => Ok(Self::new(decode_app_key(key)?)),
            _ => Err(SignedUrlError::MissingKey),
        }
    }

    /// Override the clock (UNIX seconds) for deterministic tests.
    pub fn with_clock<F>(mut self, clock: F) -> Self
    where
        F: Fn() -> i64 + Send + Sync + 'static,
    {
        self.clock = Arc::new(clock);
        self
    }

    /// Current UNIX time from this signer's clock.
    pub fn now(&self) -> i64 {
        (self.clock)()
    }
    /// Sign `path` with an absolute expiry, returning the query fragment.
    /// Infallible: HMAC accepts any key length, so an empty key yields a
    /// signature verification always rejects; use
    /// [`SignedUrlSigner::sign_expiring`] for a typed `MissingKey`.
    pub fn sign(&self, path: &str, expires_at_unix: i64, extra: &[(&str, &str)]) -> String {
        let extra_owned = owned_pairs(extra);
        let payload = canonical_payload(path, expires_at_unix, &extra_owned);
        let signature = hex::encode(self.hmac_bytes(payload.as_bytes()));

        let mut sorted = extra_owned;
        sorted.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        let mut query = String::new();
        for (key, value) in &sorted {
            query.push_str(&percent_encode(key));
            query.push('=');
            query.push_str(&percent_encode(value));
            query.push('&');
        }
        query.push_str(EXPIRES_PARAM);
        query.push('=');
        query.push_str(&expires_at_unix.to_string());
        query.push('&');
        query.push_str(SIGNATURE_PARAM);
        query.push('=');
        query.push_str(&signature);
        query
    }

    /// Sign `path` with a relative TTL, computing the expiry from the clock.
    /// Returns [`SignedUrlError::MissingKey`] when the signer's key is empty.
    pub fn sign_expiring(
        &self,
        path: &str,
        ttl_secs: u64,
        extra: &[(&str, &str)],
    ) -> Result<String, SignedUrlError> {
        if self.key.is_empty() {
            return Err(SignedUrlError::MissingKey);
        }
        let expires_at_unix = self.now().saturating_add(ttl_secs as i64);
        Ok(self.sign(path, expires_at_unix, extra))
    }

    /// Verify a signed query fragment at an explicit time: the key is present,
    /// a `signature` parameter exists, the query parses unambiguously, the HMAC
    /// matches (constant-time), and the link has not expired.
    ///
    /// # Errors
    ///
    /// [`MissingKey`](SignedUrlError::MissingKey), [`MissingSignature`]
    /// (SignedUrlError::MissingSignature), [`Malformed`](SignedUrlError::Malformed)
    /// (unparsable query, duplicate params, non-integer `expires`),
    /// [`InvalidSignature`](SignedUrlError::InvalidSignature) (mismatch, also
    /// covering a tampered `path`/`expires`/`extra`), and
    /// [`Expired`](SignedUrlError::Expired) when past the expiry.
    pub fn verify(&self, path: &str, query: &str, now_unix: i64) -> Result<(), SignedUrlError> {
        if self.key.is_empty() {
            return Err(SignedUrlError::MissingKey);
        }

        let pairs = parse_query(query)?;

        let mut signature: Option<String> = None;
        let mut expires_at_unix: Option<i64> = None;
        let mut extra: Vec<(String, String)> = Vec::new();

        for (key, value) in pairs {
            if key == SIGNATURE_PARAM {
                if signature.is_some() {
                    return Err(SignedUrlError::Malformed {
                        detail: "duplicate `signature` parameter".to_string(),
                    });
                }
                signature = Some(value);
            } else if key == EXPIRES_PARAM {
                if expires_at_unix.is_some() {
                    return Err(SignedUrlError::Malformed {
                        detail: "duplicate `expires` parameter".to_string(),
                    });
                }
                let parsed = value
                    .parse::<i64>()
                    .map_err(|_| SignedUrlError::Malformed {
                        detail: format!("`expires` is not an integer: {value:?}"),
                    })?;
                expires_at_unix = Some(parsed);
            } else {
                if extra.iter().any(|(existing, _)| existing == &key) {
                    return Err(SignedUrlError::Malformed {
                        detail: format!("duplicate parameter {key:?}"),
                    });
                }
                extra.push((key, value));
            }
        }

        let signature = signature.ok_or(SignedUrlError::MissingSignature)?;
        let expires_at_unix = expires_at_unix.ok_or_else(|| SignedUrlError::Malformed {
            detail: "missing `expires` parameter".to_string(),
        })?;

        let payload = canonical_payload(path, expires_at_unix, &extra);
        let expected = self.hmac_bytes(payload.as_bytes());

        let provided = hex::decode(&signature).map_err(|_| SignedUrlError::InvalidSignature)?;
        // Constant-time comparison: `ct_eq` inspects every byte and never
        // short-circuits on the first mismatch.
        if !bool::from(expected.as_slice().ct_eq(provided.as_slice())) {
            return Err(SignedUrlError::InvalidSignature);
        }

        if now_unix > expires_at_unix {
            return Err(SignedUrlError::Expired {
                expired_at: expires_at_unix,
                now: now_unix,
            });
        }

        Ok(())
    }

    /// Verify a signed query fragment against the signer's clock. Errors are
    /// identical to [`SignedUrlSigner::verify`].
    pub fn verify_now(&self, path: &str, query: &str) -> Result<(), SignedUrlError> {
        self.verify(path, query, self.now())
    }

    /// Compute the raw HMAC-SHA256 tag over `data`. An empty key yields an
    /// all-zero tag, which never matches a real signature (fail closed).
    fn hmac_bytes(&self, data: &[u8]) -> [u8; 32] {
        let Ok(mut mac) = HmacSha256::new_from_slice(&self.key) else {
            return [0u8; 32];
        };
        mac.update(data);
        let digest = mac.finalize().into_bytes();
        let mut tag = [0u8; 32];
        tag.copy_from_slice(&digest);
        tag
    }
}

/// Clone `extra` into owned `(String, String)` pairs.
fn owned_pairs(extra: &[(&str, &str)]) -> Vec<(String, String)> {
    extra
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Build the canonical, order-independent signing payload.
fn canonical_payload(path: &str, expires_at_unix: i64, extra: &[(String, String)]) -> String {
    let mut sorted: Vec<&(String, String)> = extra.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    let mut payload = String::new();
    payload.push_str(&percent_encode(path));
    payload.push('\n');
    payload.push_str(&expires_at_unix.to_string());
    payload.push('\n');
    for (key, value) in sorted {
        payload.push_str(&percent_encode(key));
        payload.push('=');
        payload.push_str(&percent_encode(value));
        payload.push('\n');
    }
    payload
}

/// Parse a query fragment into decoded key/value pairs. A leading `?` is
/// tolerated; empty segments, missing `=`, bad percent-escapes, and non-UTF-8
/// bytes are [`SignedUrlError::Malformed`].
fn parse_query(query: &str) -> Result<Vec<(String, String)>, SignedUrlError> {
    let query = query.strip_prefix('?').unwrap_or(query);
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let mut pairs = Vec::new();
    for segment in query.split('&') {
        if segment.is_empty() {
            return Err(SignedUrlError::Malformed {
                detail: "empty query segment".to_string(),
            });
        }
        let (key, value) = segment
            .split_once('=')
            .ok_or_else(|| SignedUrlError::Malformed {
                detail: format!("query segment missing '=': {segment:?}"),
            })?;
        pairs.push((percent_decode(key)?, percent_decode(value)?));
    }
    Ok(pairs)
}

/// Percent-encode with the RFC 3986 unreserved set preserved.
fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &byte in input.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

/// Decode `%XX` percent-escapes back into a UTF-8 string.
fn percent_decode(input: &str) -> Result<String, SignedUrlError> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 3 > bytes.len() {
                return Err(SignedUrlError::Malformed {
                    detail: format!("truncated percent-escape in {input:?}"),
                });
            }
            let high = hex_value(bytes[index + 1]).ok_or_else(|| SignedUrlError::Malformed {
                detail: format!("invalid percent-escape in {input:?}"),
            })?;
            let low = hex_value(bytes[index + 2]).ok_or_else(|| SignedUrlError::Malformed {
                detail: format!("invalid percent-escape in {input:?}"),
            })?;
            out.push((high << 4) | low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).map_err(|_| SignedUrlError::Malformed {
        detail: format!("invalid UTF-8 in {input:?}"),
    })
}

/// Value of a single hex digit.
fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Resolve an `APP_KEY` string to raw HMAC key bytes (Laravel parity).
///
/// A `base64:`-prefixed key decodes the remainder to bytes; any other key is
/// used verbatim. An invalid base64 payload is [`SignedUrlError::Malformed`],
/// never a silent fallback to the raw string.
fn decode_app_key(key: &str) -> Result<Vec<u8>, SignedUrlError> {
    match key.strip_prefix("base64:") {
        Some(payload) => base64_decode(payload).map_err(|detail| SignedUrlError::Malformed {
            detail: format!("invalid base64 in APP_KEY: {detail}"),
        }),
        None => Ok(key.as_bytes().to_vec()),
    }
}

/// Decode standard or URL-safe base64 (`=` padding optional). Returns a reason
/// on an invalid character or a truncated final group (a lone 6-bit chunk).
fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return Err(format!("invalid character {:?}", byte as char)),
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    // A valid encoding leaves 0, 2, or 4 unused bits; 6 means a lone character.
    if bits >= 6 {
        return Err("truncated base64 group".to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Percent-encoding round-trips and stays unambiguous, so `%` and `\n`
    /// cannot be smuggled into a canonical field.
    #[test]
    fn percent_encoding_round_trips() {
        let raw = "a b/c?d=e&f%g\nh";
        assert_eq!(percent_decode(&percent_encode(raw)).unwrap(), raw);
    }

    /// Signer from an `AppConfig` key, frozen at `now`.
    fn signer_for(key: &str, now: i64) -> SignedUrlSigner {
        let config = AppConfig {
            key: Some(key.to_string()),
            ..AppConfig::default()
        };
        SignedUrlSigner::from_app_config(&config)
            .unwrap()
            .with_clock(move || now)
    }

    /// A `base64:`-prefixed key signs/verifies as its decoded bytes, not the string.
    #[test]
    fn base64_prefixed_key_signs_and_verifies() {
        let signer = signer_for("base64:aGVsbG8td29ybGQ=", 1_000);
        assert_eq!(signer.key, b"hello-world");
        let url = signer.sign("/verify", 2_000, &[]);
        assert!(signer.verify("/verify", &url, 1_500).is_ok());
    }

    /// A key without the `base64:` prefix is used verbatim (backward compat).
    #[test]
    fn unprefixed_key_is_used_verbatim() {
        let signer = signer_for("plain-key", 1_000);
        assert_eq!(signer.key, b"plain-key");
        let url = signer.sign("/verify", 2_000, &[]);
        assert!(signer.verify("/verify", &url, 1_500).is_ok());
    }

    /// Invalid base64 after the prefix is a typed `Malformed`, never a silent
    /// fallback to the raw string.
    #[test]
    fn invalid_base64_key_is_malformed() {
        let config = AppConfig {
            key: Some("base64:not base64!!".to_string()),
            ..AppConfig::default()
        };
        assert!(matches!(
            SignedUrlSigner::from_app_config(&config).unwrap_err(),
            SignedUrlError::Malformed { .. }
        ));
    }
}
