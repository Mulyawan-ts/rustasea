//! Test-only helpers: an in-memory RSA key pair, service-account JSON, and an
//! independent assertion decoder (RS256 verified against the public key).

use std::sync::{Arc, OnceLock};

use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use rand::rngs::OsRng;
use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use rsa::RsaPrivateKey;

use crate::claims::AssertionClaims;

/// A generated RSA key pair rendered as PEM.
pub struct TestKeyPair {
    /// PKCS#8 PEM private key.
    pub private_pem: String,
    /// SPKI PEM public key.
    pub public_pem: String,
}

/// Return the process-wide test key pair, generating it once per test binary.
pub fn key_pair() -> Arc<TestKeyPair> {
    static PAIR: OnceLock<Arc<TestKeyPair>> = OnceLock::new();
    Arc::clone(PAIR.get_or_init(|| Arc::new(generate())))
}

/// Generate a 2048-bit RSA key pair and encode both halves as PEM.
fn generate() -> TestKeyPair {
    let mut rng = OsRng;
    let key = RsaPrivateKey::new(&mut rng, 2048).expect("generate test RSA key");
    let private_pem = key
        .to_pkcs8_pem(LineEnding::LF)
        .expect("encode test private key")
        .to_string();
    let public_pem = key
        .to_public_key()
        .to_public_key_pem(LineEnding::LF)
        .expect("encode test public key");
    TestKeyPair {
        private_pem,
        public_pem,
    }
}

/// Build a service-account JSON document embedding `private_pem`.
pub fn service_account_json(private_pem: &str) -> String {
    serde_json::json!({
        "type": "service_account",
        "project_id": "test-project",
        "private_key_id": "abc123",
        "private_key": private_pem,
        "client_email": "test@test-project.iam.gserviceaccount.com",
        "client_id": "1234567890",
        "token_uri": "https://oauth2.googleapis.com/token",
        "universe_domain": "googleapis.com",
    })
    .to_string()
}

/// Decode an assertion, verifying its RS256 signature with `public_pem`.
///
/// `aud`/`exp` are not validated so the assertion's own claims can be asserted
/// directly; the signature check is the oracle that the key was used correctly.
pub fn decode_assertion(assertion: &str, public_pem: &str) -> AssertionClaims {
    let mut validation = Validation::new(Algorithm::RS256);
    validation.validate_aud = false;
    validation.validate_exp = false;
    validation.required_spec_claims.clear();
    let key = DecodingKey::from_rsa_pem(public_pem.as_bytes()).expect("decode test public key");
    decode::<AssertionClaims>(assertion, &key, &validation)
        .expect("decode signed assertion")
        .claims
}
