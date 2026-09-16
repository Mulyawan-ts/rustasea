//! JWT assertion claims for the service-account flow (`google/auth` parity).
//!
//! A service account authenticates by signing a short-lived JWT **assertion**
//! with its RSA private key (RS256) and posting it to the token endpoint with
//! the `jwt-bearer` grant. The claim set is fixed by Google's
//! `ServiceAccountCredentials`:
//!
//! | Claim | Value |
//! | --- | --- |
//! | `iss` | service-account email |
//! | `sub` | impersonated user (domain-wide delegation), omitted when unset |
//! | `scope` | space-joined OAuth scopes |
//! | `aud` | token endpoint URL |
//! | `iat` | issued-at (seconds since the Unix epoch) |
//! | `exp` | expiry (seconds since the Unix epoch) |
//!
//! The assertion is signed with [`AssertionClaims::sign`]; the private key is
//! never stored on the claim set and never appears in an error message.

use chrono::{DateTime, Utc};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};

use crate::credentials::ServiceAccount;
use crate::error::{GoogleAuthError, Result};

/// Lifetime of a signed assertion in seconds (Google's recommended one hour).
pub const ASSERTION_LIFETIME_SECS: i64 = 3600;

/// The signed-at-the-token-endpoint JWT assertion claim set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssertionClaims {
    /// Issuer — the service-account email (`iss`).
    pub iss: String,
    /// Impersonated subject for domain-wide delegation (`sub`); omitted when
    /// the caller did not request impersonation.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub sub: Option<String>,
    /// Space-joined OAuth scopes (`scope`).
    pub scope: String,
    /// Audience — the token endpoint URL (`aud`).
    pub aud: String,
    /// Issued-at, seconds since the Unix epoch (`iat`).
    pub iat: i64,
    /// Expiry, seconds since the Unix epoch (`exp`).
    pub exp: i64,
}

impl AssertionClaims {
    /// Build the assertion claim set for a service account.
    ///
    /// `issued_at` becomes `iat`; `exp` is [`ASSERTION_LIFETIME_SECS`] later.
    /// `subject` is included as `sub` only when set (domain-wide delegation).
    #[must_use]
    pub fn for_service_account(
        account: &ServiceAccount,
        scopes: &[String],
        subject: Option<&str>,
        issued_at: DateTime<Utc>,
    ) -> Self {
        let iat = issued_at.timestamp();
        Self {
            iss: account.client_email().to_string(),
            sub: subject.map(str::to_string),
            scope: scopes.join(" "),
            aud: account.token_uri().to_string(),
            iat,
            exp: iat + ASSERTION_LIFETIME_SECS,
        }
    }

    /// Sign the claims as an RS256 JWT with the PEM private key.
    ///
    /// # Errors
    ///
    /// [`GoogleAuthError::InvalidPrivateKey`] when the PEM cannot be parsed
    /// into an RSA key, or [`GoogleAuthError::Signing`] when the JWT encoder
    /// fails. Neither message contains the key bytes.
    pub fn sign(&self, private_key_pem: &str) -> Result<String> {
        let key = EncodingKey::from_rsa_pem(private_key_pem.as_bytes()).map_err(|error| {
            GoogleAuthError::InvalidPrivateKey {
                message: error.to_string(),
            }
        })?;
        encode(&Header::new(Algorithm::RS256), self, &key).map_err(|error| {
            GoogleAuthError::Signing {
                message: error.to_string(),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{decode_assertion, key_pair, service_account_json};

    /// The signed assertion carries the expected claim set and a valid RS256
    /// signature (verified independently against the public key).
    #[test]
    fn signs_assertion_with_expected_claims() {
        let key = key_pair();
        let account = ServiceAccount::from_json(&service_account_json(&key.private_pem))
            .expect("parse service account");
        let scopes = vec![
            "https://www.googleapis.com/auth/cloud-platform".to_string(),
            "https://www.googleapis.com/auth/drive.readonly".to_string(),
        ];
        let issued_at = Utc::now();

        let claims = AssertionClaims::for_service_account(&account, &scopes, None, issued_at);
        let assertion = claims.sign(&key.private_pem).expect("sign assertion");

        let decoded = decode_assertion(&assertion, &key.public_pem);
        assert_eq!(decoded, claims);
        assert_eq!(decoded.iss, "test@test-project.iam.gserviceaccount.com");
        assert_eq!(decoded.aud, "https://oauth2.googleapis.com/token");
        assert_eq!(decoded.scope, scopes.join(" "));
        assert_eq!(decoded.exp - decoded.iat, ASSERTION_LIFETIME_SECS);
        assert!(decoded.sub.is_none());
    }

    /// Setting a subject adds the `sub` claim for domain-wide delegation.
    #[test]
    fn subject_sets_sub_claim() {
        let key = key_pair();
        let account = ServiceAccount::from_json(&service_account_json(&key.private_pem))
            .expect("parse service account");
        let claims = AssertionClaims::for_service_account(
            &account,
            &["scope-a".to_string()],
            Some("admin@example.com"),
            Utc::now(),
        );
        let assertion = claims.sign(&key.private_pem).expect("sign assertion");
        let decoded = decode_assertion(&assertion, &key.public_pem);
        assert_eq!(decoded.sub.as_deref(), Some("admin@example.com"));
    }

    /// A PEM that is not an RSA key is a typed error without key leakage.
    #[test]
    fn invalid_key_is_typed_error() {
        let key = key_pair();
        let account = ServiceAccount::from_json(&service_account_json(&key.private_pem))
            .expect("parse service account");
        let claims = AssertionClaims::for_service_account(
            &account,
            &["scope-a".to_string()],
            None,
            Utc::now(),
        );
        let secret_pem = "-----BEGIN PRIVATE KEY-----\nnot-a-key\n-----END PRIVATE KEY-----";
        let error = claims.sign(secret_pem).expect_err("bad key must fail");
        assert!(matches!(error, GoogleAuthError::InvalidPrivateKey { .. }));
        assert!(!error.to_string().contains("not-a-key"));
    }
}
