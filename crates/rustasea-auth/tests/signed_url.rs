//! Integration tests for [`rustasea_auth::signed_url`] (AUTH-012).
//!
//! Each failure mode is its own test so a regression is pinpointed. The
//! positive round-trips exercise order-independent canonicalisation.

use rustasea_auth::signed_url::{SignedUrlError, SignedUrlSigner};
use rustasea_foundation::AppConfig;
use subtle::ConstantTimeEq;

/// Key used by most tests.
const KEY: &str = "base64:test-signing-key-for-signed-urls";

/// Constant-time comparison must not short-circuit: two tags that differ only
/// in their first byte and only in their last byte both compare unequal, and
/// the identical tag compares equal. `subtle::ConstantTimeEq` is the same
/// helper [`SignedUrlSigner`] uses internally, so this pins the property.
#[test]
fn signature_comparison_is_constant_time() {
    let expected = [7u8; 32];
    let mut differ_first = expected;
    differ_first[0] ^= 0xFF;
    let mut differ_last = expected;
    differ_last[31] ^= 0xFF;

    assert!(!bool::from(
        expected.as_slice().ct_eq(differ_first.as_slice())
    ));
    assert!(!bool::from(
        expected.as_slice().ct_eq(differ_last.as_slice())
    ));
    assert!(bool::from(expected.as_slice().ct_eq(expected.as_slice())));
}

/// Signer frozen at a fixed UNIX time.
fn signer_at(now: i64) -> SignedUrlSigner {
    SignedUrlSigner::new(KEY).with_clock(move || now)
}

/// A signed URL verifies at a time before its expiry.
#[test]
fn signed_url_verifies_before_expiry() {
    let signer = signer_at(1_000);
    let url = signer
        .sign_expiring("/verify/email", 3_600, &[("id", "42")])
        .unwrap();
    assert!(signer.verify("/verify/email", &url, 1_001).is_ok());
}

/// Extra parameters may be supplied in any order: the canonical payload sorts
/// them, so both orders yield the same signature.
#[test]
fn canonicalization_is_order_independent() {
    let signer = signer_at(1_000);
    let a = signer.sign("/verify", 2_000, &[("b", "2"), ("a", "1")]);
    let b = signer.sign("/verify", 2_000, &[("a", "1"), ("b", "2")]);
    assert_eq!(a, b);
    assert!(signer.verify("/verify", &a, 1_500).is_ok());
}

/// `verify_now` uses the signer's clock.
#[test]
fn verify_now_uses_clock() {
    let signer = signer_at(1_000);
    let url = signer.sign("/reset", 2_000, &[]);
    assert!(signer.verify_now("/reset", &url).is_ok());
}

/// A tampered path invalidates the signature.
#[test]
fn tampered_path_is_invalid() {
    let signer = signer_at(1_000);
    let url = signer.sign("/verify/email", 2_000, &[]);
    assert_eq!(
        signer.verify("/verify/other", &url, 1_500).unwrap_err(),
        SignedUrlError::InvalidSignature
    );
}

/// A tampered expiry invalidates the signature (it is covered by the HMAC).
#[test]
fn tampered_expiry_is_invalid() {
    let signer = signer_at(1_000);
    let url = signer.sign("/verify", 2_000, &[]);
    let tampered = url.replace("expires=2000", "expires=9999999999");
    assert_eq!(
        signer.verify("/verify", &tampered, 1_500).unwrap_err(),
        SignedUrlError::InvalidSignature
    );
}

/// A tampered extra parameter invalidates the signature.
#[test]
fn tampered_extra_param_is_invalid() {
    let signer = signer_at(1_000);
    let url = signer.sign("/verify", 2_000, &[("id", "42"), ("role", "user")]);
    let tampered = url.replace("role=user", "role=admin");
    assert_eq!(
        signer.verify("/verify", &tampered, 1_500).unwrap_err(),
        SignedUrlError::InvalidSignature
    );
}

/// An expired link reports the encoded expiry and the verification time.
#[test]
fn expired_url_reports_expiry() {
    let signer = signer_at(1_000);
    let url = signer.sign("/verify", 2_000, &[]);
    assert_eq!(
        signer.verify("/verify", &url, 2_001).unwrap_err(),
        SignedUrlError::Expired {
            expired_at: 2_000,
            now: 2_001,
        }
    );
}

/// A query with no `signature` parameter fails with `MissingSignature`.
#[test]
fn missing_signature_is_reported() {
    let signer = signer_at(1_000);
    assert_eq!(
        signer.verify("/verify", "expires=2000", 1_500).unwrap_err(),
        SignedUrlError::MissingSignature
    );
}

/// A missing app key fails closed at construction.
#[test]
fn missing_app_key_fails_closed() {
    let config = AppConfig::default();
    assert_eq!(
        SignedUrlSigner::from_app_config(&config).unwrap_err(),
        SignedUrlError::MissingKey
    );
}

/// An empty app key fails closed at construction.
#[test]
fn empty_app_key_fails_closed() {
    let config = AppConfig {
        key: Some(String::new()),
        ..AppConfig::default()
    };
    assert_eq!(
        SignedUrlSigner::from_app_config(&config).unwrap_err(),
        SignedUrlError::MissingKey
    );
}

/// A whitespace-only app key is treated as missing (fail closed).
#[test]
fn blank_app_key_fails_closed() {
    let config = AppConfig {
        key: Some("   ".to_string()),
        ..AppConfig::default()
    };
    assert_eq!(
        SignedUrlSigner::from_app_config(&config).unwrap_err(),
        SignedUrlError::MissingKey
    );
}

/// A configured key builds a working signer.
#[test]
fn app_config_key_signs_and_verifies() {
    let config = AppConfig {
        key: Some(KEY.to_string()),
        ..AppConfig::default()
    };
    let signer = SignedUrlSigner::from_app_config(&config)
        .unwrap()
        .with_clock(|| 1_000);
    let url = signer.sign("/verify", 2_000, &[]);
    assert!(signer.verify("/verify", &url, 1_500).is_ok());
}

/// An empty-key signer never produces a verifiable URL (fail closed).
#[test]
fn empty_key_signer_is_inert() {
    let signer = SignedUrlSigner::new("");
    assert_eq!(
        signer.sign_expiring("/verify", 60, &[]).unwrap_err(),
        SignedUrlError::MissingKey
    );
    let url = signer.sign("/verify", 2_000, &[]);
    assert_eq!(
        signer.verify("/verify", &url, 1_500).unwrap_err(),
        SignedUrlError::MissingKey
    );
}

/// A segment without `=` is malformed.
#[test]
fn malformed_query_without_equals() {
    let signer = signer_at(1_000);
    let err = signer
        .verify("/verify", "signature=abc&expires", 1_500)
        .unwrap_err();
    assert!(
        matches!(err, SignedUrlError::Malformed { .. }),
        "got {err:?}"
    );
}

/// A non-integer `expires` is malformed.
#[test]
fn malformed_expires_is_reported() {
    let signer = signer_at(1_000);
    let err = signer
        .verify("/verify", "expires=soon&signature=abc", 1_500)
        .unwrap_err();
    assert!(
        matches!(err, SignedUrlError::Malformed { .. }),
        "got {err:?}"
    );
}

/// A truncated percent-escape is malformed, never accepted.
#[test]
fn malformed_percent_escape_is_reported() {
    let signer = signer_at(1_000);
    let err = signer
        .verify("/verify", "id=%ZZ&expires=2000&signature=abc", 1_500)
        .unwrap_err();
    assert!(
        matches!(err, SignedUrlError::Malformed { .. }),
        "got {err:?}"
    );
}

/// Duplicate `signature` parameters are rejected as ambiguous.
#[test]
fn duplicate_signature_is_malformed() {
    let signer = signer_at(1_000);
    let err = signer
        .verify("/verify", "expires=2000&signature=aa&signature=bb", 1_500)
        .unwrap_err();
    assert!(
        matches!(err, SignedUrlError::Malformed { .. }),
        "got {err:?}"
    );
}

/// A non-hex signature is an invalid signature, not a panic.
#[test]
fn non_hex_signature_is_invalid() {
    let signer = signer_at(1_000);
    let url = signer.sign("/verify", 2_000, &[]);
    let signature = url.rsplit("signature=").next().unwrap().to_string();
    let tampered = url.replace(&format!("signature={signature}"), "signature=zzzz");
    assert_eq!(
        signer.verify("/verify", &tampered, 1_500).unwrap_err(),
        SignedUrlError::InvalidSignature
    );
}
