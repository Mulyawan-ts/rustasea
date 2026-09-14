//! Hand-rolled WebAuthn ceremony primitives (registration + assertion).
//!
//! RustaSea deliberately hand-rolls the WebAuthn ceremony rules rather than
//! pulling a full relying-party crate: the surface needed here is small and
//! stable (W3C WebAuthn Level 2 §6.1 registration and §6.3 assertion), and a
//! hand-rolled verifier keeps the dependency graph lean. The rules implemented
//! are:
//!
//! * `clientDataJSON` ceremony `type`, `challenge`, and `origin` checks;
//! * the SHA-256 relying-party id hash over `authenticatorData`;
//! * the user-present flag;
//! * COSE (`kty=EC2`, `crv=P-256`, `alg=ES256`) public-key extraction to SEC1;
//! * attestation format `none` and `packed` (self-attestation; an `x5c`
//!   attestation chain is accepted but **not** validated — see below);
//! * ES256 assertion signature verification (ASN.1 DER or raw `r||s`).
//!
//! # Attestation trust
//!
//! Only self-attestation is in scope. `fmt = "none"` and `fmt = "packed"` are
//! accepted; a `packed` statement carrying an `x5c` certificate chain is
//! accepted **without** validating that chain against a trust store, so the
//! attestation is treated as self-attested. Attestation is not used for
//! authorization here (the account is already authenticated when registering),
//! so this does not weaken the login boundary. Any other format is rejected.
//!
//! # Signature counter
//!
//! Counter regression is checked by the [`super::PasskeyService`], not here, so
//! this module stays a pure verifier and the service owns the stored state.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use ciborium::value::Value as CborValue;
use hmac::{Hmac, Mac};
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{AuthError, Result};

pub use crate::passkeys::types::*;

/// COSE algorithm identifier for ECDSA with SHA-256 (ES256).
pub const ES256_ALG: i64 = -7;

/// COSE key type for elliptic-curve keys.
const COSE_KTY_EC2: i128 = 2;

/// COSE curve identifier for NIST P-256.
const COSE_CRV_P256: i128 = 1;

/// Authenticator-data flag: user present.
const FLAG_USER_PRESENT: u8 = 0x01;

/// Authenticator-data flag: attested credential data included.
const FLAG_ATTESTED_CREDENTIAL_DATA: u8 = 0x40;

/// Length of the SHA-256 relying-party id hash.
const RP_ID_HASH_LEN: usize = 32;

/// Length of the AAGUID in attested credential data.
const AAGUID_LEN: usize = 16;

/// Fixed authenticator-data header length (`rpIdHash | flags | signCount`).
const AUTH_DATA_HEADER_LEN: usize = RP_ID_HASH_LEN + 1 + 4;

/// Bytes in a raw P-256 coordinate.
const P256_COORD_LEN: usize = 32;

/// Bytes in an uncompressed SEC1 P-256 point (`0x04 || x || y`).
const P256_POINT_LEN: usize = 1 + P256_COORD_LEN * 2;

/// Registration ceremony `clientDataJSON` type.
const CEREMONY_CREATE: &str = "webauthn.create";

/// Assertion ceremony `clientDataJSON` type.
const CEREMONY_GET: &str = "webauthn.get";

/// Parsed `clientDataJSON`.
#[derive(Debug, Deserialize)]
struct ClientData {
    /// Ceremony type (`webauthn.create` / `webauthn.get`).
    #[serde(rename = "type")]
    ceremony_type: String,
    /// Challenge echoed by the client (base64url).
    challenge: String,
    /// Origin that produced the response.
    origin: String,
}

/// Parsed authenticator data.
struct AuthData<'a> {
    /// SHA-256 relying-party id hash.
    rp_id_hash: &'a [u8],
    /// Authenticator-data flags.
    flags: u8,
    /// Signature counter.
    counter: u32,
    /// Credential id, when attested credential data is present.
    credential_id: Option<Vec<u8>>,
    /// COSE public key bytes, when attested credential data is present.
    cose_key: Option<&'a [u8]>,
}

/// Generate a fresh 256-bit challenge (base64url, no padding).
pub fn generate_challenge() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Derive the opaque WebAuthn user handle for `user_id` (base64url HMAC-SHA256).
///
/// The handle is a keyed digest, so it is stable for a user across ceremonies
/// (discoverable credentials need a stable handle) without exposing the raw id.
pub fn user_handle(secret: &str, user_id: &str) -> Result<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|_| AuthError::Passkey("invalid user-handle secret".into()))?;
    mac.update(user_id.as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

/// Verify a registration response and extract the stored credential material.
///
/// # Errors
///
/// [`AuthError::Passkey`] when the response is malformed, the ceremony checks
/// fail, the attestation format is unsupported, or the COSE key is not an
/// ES256 P-256 key.
pub fn verify_registration(
    relying_party_id: &str,
    allowed_origins: &[String],
    expected_challenge: &str,
    response: &RegistrationResponse,
) -> Result<RegistrationOutcome> {
    let client_data_bytes =
        decode_base64url("clientDataJSON", &response.response.client_data_json)?;
    let client_data: ClientData = serde_json::from_slice(&client_data_bytes)
        .map_err(|_| AuthError::Passkey("clientDataJSON is not valid JSON".into()))?;
    check_client_data(
        &client_data,
        CEREMONY_CREATE,
        allowed_origins,
        expected_challenge,
    )?;

    let attestation_bytes =
        decode_base64url("attestationObject", &response.response.attestation_object)?;
    let attestation: AttestationObject = ciborium::de::from_reader(&attestation_bytes[..])
        .map_err(|_| AuthError::Passkey("attestationObject is not valid CBOR".into()))?;

    let auth_data = parse_auth_data(&attestation.auth_data)?;
    check_rp_id_hash(auth_data.rp_id_hash, relying_party_id)?;
    if auth_data.flags & FLAG_USER_PRESENT == 0 {
        return Err(AuthError::Passkey("user-present flag is not set".into()));
    }
    if auth_data.flags & FLAG_ATTESTED_CREDENTIAL_DATA == 0 {
        return Err(AuthError::Passkey(
            "attested credential data is missing".into(),
        ));
    }
    let credential_id = auth_data
        .credential_id
        .clone()
        .ok_or_else(|| AuthError::Passkey("credential id is missing".into()))?;
    let cose_key = auth_data
        .cose_key
        .ok_or_else(|| AuthError::Passkey("COSE public key is missing".into()))?;
    let public_key = cose_to_sec1(cose_key)?;

    verify_attestation(
        &attestation,
        &public_key,
        &attestation.auth_data,
        &sha256(&client_data_bytes),
    )?;

    Ok(RegistrationOutcome {
        credential_id: URL_SAFE_NO_PAD.encode(credential_id),
        public_key,
        counter: auth_data.counter,
    })
}

/// Verify an assertion response against the stored credential public key.
///
/// # Errors
///
/// [`AuthError::InvalidPasskeyAssertion`] when the ceremony checks or the
/// signature fail; [`AuthError::Passkey`] when the response is malformed.
pub fn verify_assertion(
    relying_party_id: &str,
    allowed_origins: &[String],
    expected_challenge: &str,
    stored_public_key: &[u8],
    response: &AuthenticationResponse,
) -> Result<AssertionOutcome> {
    let client_data_bytes =
        decode_base64url("clientDataJSON", &response.response.client_data_json)?;
    let client_data: ClientData = serde_json::from_slice(&client_data_bytes)
        .map_err(|_| AuthError::Passkey("clientDataJSON is not valid JSON".into()))?;
    check_client_data(
        &client_data,
        CEREMONY_GET,
        allowed_origins,
        expected_challenge,
    )?;

    let auth_data_bytes =
        decode_base64url("authenticatorData", &response.response.authenticator_data)?;
    let auth_data = parse_auth_data(&auth_data_bytes)?;
    check_rp_id_hash(auth_data.rp_id_hash, relying_party_id)?;
    if auth_data.flags & FLAG_USER_PRESENT == 0 {
        return Err(AuthError::InvalidPasskeyAssertion);
    }

    let mut message = auth_data_bytes.clone();
    message.extend_from_slice(&sha256(&client_data_bytes));
    let signature = decode_base64url("signature", &response.response.signature)?;
    verify_signature(stored_public_key, &message, &signature)
        .map_err(|_| AuthError::InvalidPasskeyAssertion)?;

    let user_handle = match response.response.user_handle.as_deref() {
        Some(value) if !value.is_empty() => Some(value.to_string()),
        _ => None,
    };
    Ok(AssertionOutcome {
        counter: auth_data.counter,
        user_handle,
    })
}

/// Decoded CBOR attestation object.
#[derive(Debug, Deserialize)]
struct AttestationObject {
    /// Attestation statement format identifier.
    fmt: String,
    /// Attestation statement (format-specific).
    #[serde(rename = "attStmt")]
    att_stmt: CborValue,
    /// Raw authenticator data.
    #[serde(rename = "authData")]
    auth_data: Vec<u8>,
}

/// Validate the ceremony `type`, `challenge`, and `origin` in `clientDataJSON`.
fn check_client_data(
    client_data: &ClientData,
    expected_type: &str,
    allowed_origins: &[String],
    expected_challenge: &str,
) -> Result<()> {
    if client_data.ceremony_type != expected_type {
        return Err(AuthError::Passkey(format!(
            "unexpected ceremony type {:?}",
            client_data.ceremony_type
        )));
    }
    if client_data.challenge != expected_challenge {
        return Err(AuthError::Passkey("challenge mismatch".into()));
    }
    if !allowed_origins
        .iter()
        .any(|origin| origin == &client_data.origin)
    {
        return Err(AuthError::Passkey("origin is not allowed".into()));
    }
    Ok(())
}

/// Parse the fixed authenticator-data header and optional attested data.
fn parse_auth_data(bytes: &[u8]) -> Result<AuthData<'_>> {
    if bytes.len() < AUTH_DATA_HEADER_LEN {
        return Err(AuthError::Passkey("authenticatorData is too short".into()));
    }
    let rp_id_hash = &bytes[..RP_ID_HASH_LEN];
    let flags = bytes[RP_ID_HASH_LEN];
    let counter = u32::from_be_bytes([
        bytes[RP_ID_HASH_LEN + 1],
        bytes[RP_ID_HASH_LEN + 2],
        bytes[RP_ID_HASH_LEN + 3],
        bytes[RP_ID_HASH_LEN + 4],
    ]);

    if flags & FLAG_ATTESTED_CREDENTIAL_DATA == 0 {
        return Ok(AuthData {
            rp_id_hash,
            flags,
            counter,
            credential_id: None,
            cose_key: None,
        });
    }

    let mut cursor = AUTH_DATA_HEADER_LEN;
    if bytes.len() < cursor + AAGUID_LEN + 2 {
        return Err(AuthError::Passkey(
            "attested credential data is truncated".into(),
        ));
    }
    cursor += AAGUID_LEN;
    let id_len = usize::from(u16::from_be_bytes([bytes[cursor], bytes[cursor + 1]]));
    cursor += 2;
    if bytes.len() < cursor + id_len {
        return Err(AuthError::Passkey("credential id is truncated".into()));
    }
    let credential_id = bytes[cursor..cursor + id_len].to_vec();
    cursor += id_len;
    if bytes.len() <= cursor {
        return Err(AuthError::Passkey("COSE public key is missing".into()));
    }
    Ok(AuthData {
        rp_id_hash,
        flags,
        counter,
        credential_id: Some(credential_id),
        cose_key: Some(&bytes[cursor..]),
    })
}

/// Check the authenticator-data relying-party id hash against `relying_party_id`.
fn check_rp_id_hash(actual: &[u8], relying_party_id: &str) -> Result<()> {
    if actual != sha256(relying_party_id.as_bytes()) {
        return Err(AuthError::Passkey("relying-party id hash mismatch".into()));
    }
    Ok(())
}

/// Verify the attestation statement for the accepted self-attestation formats.
fn verify_attestation(
    attestation: &AttestationObject,
    public_key: &[u8],
    auth_data: &[u8],
    client_data_hash: &[u8],
) -> Result<()> {
    match attestation.fmt.as_str() {
        "none" => Ok(()),
        "packed" => verify_packed(attestation, public_key, auth_data, client_data_hash),
        other => Err(AuthError::Passkey(format!(
            "unsupported attestation format {other:?}"
        ))),
    }
}

/// Verify a `packed` attestation statement (self-attestation only).
///
/// A statement carrying an `x5c` chain is accepted without validating the
/// chain (see the module docs); a self-attestation is verified with the
/// credential public key.
fn verify_packed(
    attestation: &AttestationObject,
    public_key: &[u8],
    auth_data: &[u8],
    client_data_hash: &[u8],
) -> Result<()> {
    let Some(fields) = attestation.att_stmt.as_map() else {
        return Err(AuthError::Passkey(
            "packed attestation statement is malformed".into(),
        ));
    };
    if fields.iter().any(|(key, _)| key.as_text() == Some("x5c")) {
        // Attestation trust is out of scope; accept the statement as-is.
        return Ok(());
    }
    let signature = fields
        .iter()
        .find(|(key, _)| key.as_text() == Some("sig"))
        .and_then(|(_, value)| value.as_bytes())
        .ok_or_else(|| AuthError::Passkey("packed attestation has no signature".into()))?;
    let mut message = auth_data.to_vec();
    message.extend_from_slice(client_data_hash);
    verify_signature(public_key, &message, signature)
}

/// Extract the SEC1 uncompressed point from a COSE EC2/P-256 public key.
fn cose_to_sec1(cose_key: &[u8]) -> Result<Vec<u8>> {
    let value: CborValue = ciborium::de::from_reader(cose_key)
        .map_err(|_| AuthError::Passkey("COSE public key is not valid CBOR".into()))?;
    let fields = value
        .as_map()
        .ok_or_else(|| AuthError::Passkey("COSE public key is not a map".into()))?;

    let mut kty = None;
    let mut crv = None;
    let mut x = None;
    let mut y = None;
    for (key, value) in fields {
        let Some(label) = key.as_integer().map(i128::from) else {
            continue;
        };
        match label {
            1 => kty = value.as_integer().map(i128::from),
            -1 => crv = value.as_integer().map(i128::from),
            -2 => x = value.as_bytes().cloned(),
            -3 => y = value.as_bytes().cloned(),
            _ => {}
        }
    }
    if kty != Some(COSE_KTY_EC2) || crv != Some(COSE_CRV_P256) {
        return Err(AuthError::Passkey(
            "COSE key is not an EC2 P-256 key".into(),
        ));
    }
    let x = x.ok_or_else(|| AuthError::Passkey("COSE key is missing x".into()))?;
    let y = y.ok_or_else(|| AuthError::Passkey("COSE key is missing y".into()))?;
    if x.len() != P256_COORD_LEN || y.len() != P256_COORD_LEN {
        return Err(AuthError::Passkey(
            "COSE key coordinates are not 32 bytes".into(),
        ));
    }
    let mut sec1 = Vec::with_capacity(P256_POINT_LEN);
    sec1.push(0x04);
    sec1.extend_from_slice(&x);
    sec1.extend_from_slice(&y);
    Ok(sec1)
}

/// Verify an ES256 signature (ASN.1 DER or raw `r||s`) over `message`.
fn verify_signature(public_key: &[u8], message: &[u8], signature: &[u8]) -> Result<()> {
    let verifying_key = VerifyingKey::from_sec1_bytes(public_key)
        .map_err(|_| AuthError::Passkey("stored public key is invalid".into()))?;
    let parsed = Signature::from_der(signature)
        .or_else(|_| Signature::from_slice(signature))
        .map_err(|_| AuthError::Passkey("signature is malformed".into()))?;
    verifying_key
        .verify(message, &parsed)
        .map_err(|_| AuthError::Passkey("signature verification failed".into()))
}

/// Decode a base64url (unpadded) ceremony field.
fn decode_base64url(field: &str, value: &str) -> Result<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| AuthError::Passkey(format!("{field} is not valid base64url")))
}

/// SHA-256 digest of `bytes`.
fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
