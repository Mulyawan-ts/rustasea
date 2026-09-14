//! WebAuthn data-transfer objects (options + browser responses).
//!
//! Split out of [`super::webauthn`] for the 500-line cap. These are pure serde
//! DTOs — the ceremony logic stays in [`super::webauthn`]. Field names mirror
//! the W3C WebAuthn JSON wire format exactly (including the all-caps `JSON` in
//! `clientDataJSON`, which serde's `camelCase` cannot produce).

use serde::{Deserialize, Serialize};

/// Relying-party descriptor in creation options.
#[derive(Debug, Clone, Serialize)]
pub struct RelyingParty {
    /// Relying-party id (the effective domain).
    pub id: String,
    /// Human-readable relying-party name.
    pub name: String,
}

/// User descriptor in creation options.
#[derive(Debug, Clone, Serialize)]
pub struct UserEntity {
    /// Opaque user handle (base64url), derived from the user id.
    pub id: String,
    /// Login account name (usually the email).
    pub name: String,
    /// Display name shown by the authenticator.
    pub display_name: String,
}

/// One accepted public-key credential parameter.
#[derive(Debug, Clone, Serialize)]
pub struct PubKeyCredParam {
    /// Credential type; always `public-key`.
    #[serde(rename = "type")]
    pub credential_type: &'static str,
    /// COSE algorithm identifier.
    pub alg: i64,
}

/// A credential descriptor used to exclude or allow credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialDescriptor {
    /// Credential type; always `public-key`.
    #[serde(rename = "type")]
    pub credential_type: String,
    /// Credential id (base64url).
    pub id: String,
}

/// `PublicKeyCredentialCreationOptions` returned by registration begin.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicKeyCredentialCreationOptions {
    /// Random challenge (base64url).
    pub challenge: String,
    /// Relying-party descriptor.
    pub rp: RelyingParty,
    /// User descriptor.
    pub user: UserEntity,
    /// Accepted credential algorithms.
    pub pub_key_cred_params: Vec<PubKeyCredParam>,
    /// Ceremony timeout in milliseconds.
    pub timeout: u64,
    /// Attestation conveyance preference.
    pub attestation: &'static str,
    /// Credentials the authenticator must not re-register.
    pub exclude_credentials: Vec<CredentialDescriptor>,
}

/// `PublicKeyCredentialRequestOptions` returned by authentication begin.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicKeyCredentialRequestOptions {
    /// Random challenge (base64url).
    pub challenge: String,
    /// Relying-party id.
    pub rp_id: String,
    /// Ceremony timeout in milliseconds.
    pub timeout: u64,
    /// Credentials the authenticator may use.
    pub allow_credentials: Vec<CredentialDescriptor>,
    /// User-verification preference.
    pub user_verification: &'static str,
}

/// Registration credential payload returned by the browser.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationResponse {
    /// Credential id (base64url).
    pub id: String,
    /// Raw credential id (base64url); ignored when absent.
    #[serde(default)]
    pub raw_id: Option<String>,
    /// Optional user-chosen credential label.
    #[serde(default)]
    pub name: Option<String>,
    /// Attestation response.
    pub response: RegistrationResponseInner,
}

/// Inner attestation response.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationResponseInner {
    /// Base64url `clientDataJSON`.
    #[serde(rename = "clientDataJSON")]
    pub client_data_json: String,
    /// Base64url CBOR `attestationObject`.
    pub attestation_object: String,
}

/// Assertion credential payload returned by the browser.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticationResponse {
    /// Credential id (base64url).
    pub id: String,
    /// Raw credential id (base64url); ignored when absent.
    #[serde(default)]
    pub raw_id: Option<String>,
    /// Assertion response.
    pub response: AuthenticationResponseInner,
}

/// Inner assertion response.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticationResponseInner {
    /// Base64url `clientDataJSON`.
    #[serde(rename = "clientDataJSON")]
    pub client_data_json: String,
    /// Base64url `authenticatorData`.
    pub authenticator_data: String,
    /// Base64url signature.
    pub signature: String,
    /// Base64url user handle, or `None` for a non-discoverable credential.
    #[serde(default)]
    pub user_handle: Option<String>,
}

/// Verified registration material extracted from an attestation object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationOutcome {
    /// Credential id (base64url).
    pub credential_id: String,
    /// SEC1 uncompressed P-256 public key.
    pub public_key: Vec<u8>,
    /// Signature counter reported at registration.
    pub counter: u32,
}

/// Verified assertion material extracted from an authentication response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionOutcome {
    /// Signature counter reported by the authenticator.
    pub counter: u32,
    /// User handle (base64url), or `None`.
    pub user_handle: Option<String>,
}
