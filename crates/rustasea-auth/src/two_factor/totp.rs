//! TOTP (RFC 6238) primitives — secret minting, provisioning URI, verification.
//!
//! The implementation is deliberately dependency-light: HMAC-SHA1 over the
//! workspace `hmac` + `sha1` crates plus a hand-rolled RFC 4648 base32 codec.
//! SHA-1 is the RFC 6238 default and the algorithm every authenticator app
//! implements, so a secret minted here scans into Google Authenticator,
//! 1Password, Aegis, and friends without an algorithm hint in the URI.
//!
//! Time is always supplied by the caller (`now_secs`) rather than read from the
//! clock, so verification is deterministic and testable; the service layer
//! passes the wall clock in.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use subtle::ConstantTimeEq;

/// HMAC-SHA1 instantiation used by HOTP.
type HmacSha1 = Hmac<Sha1>;

/// Secret length in bytes (160-bit, RFC 4226 §4 recommended minimum).
pub const SECRET_BYTES: usize = 20;

/// Number of digits in a generated code.
pub const DIGITS: u32 = 6;

/// Time step in seconds (RFC 6238 default).
pub const STEP_SECS: u64 = 30;

/// Default verification window (steps accepted either side of "now").
pub const DEFAULT_WINDOW: u32 = 1;

/// RFC 4648 base32 alphabet (uppercase, no padding).
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Generate a random base32 TOTP secret (20 bytes / 160 bits).
pub fn generate_secret() -> String {
    let mut bytes = [0u8; SECRET_BYTES];
    OsRng.fill_bytes(&mut bytes);
    base32_encode(&bytes)
}

/// Encode `data` as unpadded uppercase base32 (RFC 4648).
pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for &byte in data {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            let index = ((buffer >> bits) & 0x1f) as usize;
            out.push(BASE32_ALPHABET[index] as char);
        }
    }
    if bits > 0 {
        let index = ((buffer << (5 - bits)) & 0x1f) as usize;
        out.push(BASE32_ALPHABET[index] as char);
    }
    out
}

/// Decode an unpadded base32 string; `None` on any non-alphabet byte.
pub fn base32_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 5 / 8);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for byte in input.bytes() {
        let value = decode_symbol(byte)?;
        buffer = (buffer << 5) | u32::from(value);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// Map one base32 symbol (case-insensitive) to its 5-bit value.
fn decode_symbol(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a'),
        b'2'..=b'7' => Some(byte - b'2' + 26),
        _ => None,
    }
}

/// Build the `otpauth://totp/...` provisioning URI for an authenticator app.
///
/// The label is `{issuer}:{account}` (both percent-encoded) and the issuer is
/// repeated as a query parameter, matching the Key URI Format every
/// authenticator understands. No `algorithm`/`digits`/`period` parameters are
/// emitted because SHA-1 / 6 digits / 30 seconds are the specification
/// defaults.
pub fn otpauth_uri(issuer: &str, account: &str, secret: &str) -> String {
    // The label separator is a literal `:`, so issuer and account are encoded
    // independently (encoding the whole label would escape the separator).
    let label = format!("{}:{}", percent_encode(issuer), percent_encode(account));
    format!(
        "otpauth://totp/{label}?secret={secret}&issuer={}",
        percent_encode(issuer),
    )
}

/// Percent-encode `input`, preserving only the RFC 3986 unreserved set.
fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &byte in input.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Compute the RFC 4226 HOTP value for `counter` with `digits` digits.
fn hotp(secret: &[u8], counter: u64, digits: u32) -> Option<u32> {
    let mut mac = HmacSha1::new_from_slice(secret).ok()?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    // Dynamic truncation (RFC 4226 §5.3): the low nibble of the last byte
    // selects a 4-byte window whose high bit is masked off.
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let binary = (u32::from(digest[offset] & 0x7f) << 24)
        | (u32::from(digest[offset + 1]) << 16)
        | (u32::from(digest[offset + 2]) << 8)
        | u32::from(digest[offset + 3]);
    Some(binary % 10u32.pow(digits))
}

/// Format an HOTP value as a zero-padded `digits`-wide string.
fn format_code(value: u32, digits: u32) -> String {
    format!("{value:0width$}", width = digits as usize)
}

/// Compute the TOTP code for `secret_b32` at `now_secs`.
///
/// Returns `None` when the secret is not valid base32. Primarily a test and
/// provisioning aid; production verification goes through [`verify_code`].
pub fn code_at(secret_b32: &str, now_secs: u64) -> Option<String> {
    let secret = base32_decode(secret_b32)?;
    let value = hotp(&secret, now_secs / STEP_SECS, DIGITS)?;
    Some(format_code(value, DIGITS))
}

/// Verify `code` against `secret_b32`, accepting `window` steps either side.
///
/// A non-6-digit code, an unparsable secret, or a code outside the window all
/// return `false`; the comparison is constant-time. A `window` of `1` accepts
/// the previous, current, and next 30-second step — the common clock-skew
/// tolerance.
pub fn verify_code(secret_b32: &str, code: &str, window: u32, now_secs: u64) -> bool {
    let code = code.trim();
    if code.len() != DIGITS as usize || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let Some(secret) = base32_decode(secret_b32) else {
        return false;
    };
    let counter = (now_secs / STEP_SECS) as i64;
    let window = i64::from(window);
    for delta in -window..=window {
        let candidate = counter + delta;
        if candidate < 0 {
            continue;
        }
        if let Some(value) = hotp(&secret, candidate as u64, DIGITS) {
            let candidate_code = format_code(value, DIGITS);
            if bool::from(candidate_code.as_bytes().ct_eq(code.as_bytes())) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Base32 round-trips arbitrary bytes, including non-multiples of five.
    #[test]
    fn base32_round_trip() {
        for length in 0..40usize {
            let bytes: Vec<u8> = (0..length).map(|index| (index * 7 + 1) as u8).collect();
            let encoded = base32_encode(&bytes);
            assert_eq!(
                base32_decode(&encoded).as_deref(),
                Some(bytes.as_slice()),
                "length {length}"
            );
        }
    }

    /// A known RFC 4648 vector decodes correctly and rejects bad symbols.
    #[test]
    fn base32_known_vector_and_rejection() {
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
        assert_eq!(
            base32_decode("MZXW6YTBOI").as_deref(),
            Some(b"foobar".as_ref())
        );
        assert!(base32_decode("0189!").is_none());
    }

    /// A generated secret is valid base32 and verifies its own current code.
    #[test]
    fn generated_secret_verifies_current_code() {
        let secret = generate_secret();
        assert_eq!(secret.len(), 32, "20 bytes encode to 32 base32 chars");
        let now = 1_700_000_000u64;
        let code = code_at(&secret, now).expect("code for a valid secret");
        assert!(verify_code(&secret, &code, DEFAULT_WINDOW, now));
    }

    /// The window accepts adjacent steps but rejects far-away codes.
    #[test]
    fn window_accepts_skew_and_rejects_remote_steps() {
        let secret = generate_secret();
        let now = 1_700_000_000u64;
        let previous = code_at(&secret, now - STEP_SECS).expect("previous step");
        let next = code_at(&secret, now + STEP_SECS).expect("next step");
        let far = code_at(&secret, now - 5 * STEP_SECS).expect("remote step");
        assert!(verify_code(&secret, &previous, DEFAULT_WINDOW, now));
        assert!(verify_code(&secret, &next, DEFAULT_WINDOW, now));
        assert!(!verify_code(&secret, &far, DEFAULT_WINDOW, now));
    }

    /// Malformed codes and secrets fail closed.
    #[test]
    fn malformed_inputs_fail_closed() {
        let secret = generate_secret();
        assert!(!verify_code(&secret, "12345", DEFAULT_WINDOW, 0));
        assert!(!verify_code(&secret, "abcdef", DEFAULT_WINDOW, 0));
        assert!(!verify_code("0189!", "123456", DEFAULT_WINDOW, 0));
    }

    /// The provisioning URI carries the encoded label and issuer.
    #[test]
    fn otpauth_uri_shape() {
        let uri = otpauth_uri("RustaSea", "ada@example.com", "ABCDEF");
        assert_eq!(
            uri,
            "otpauth://totp/RustaSea:ada%40example.com?secret=ABCDEF&issuer=RustaSea"
        );
    }
}
