//! Passkeys / WebAuthn (AUTH-017) — registration, authentication, management.
//!
//! This module is the RustaSea analogue of Laravel Fortify's passkey feature:
//! it drives the two WebAuthn ceremonies end to end over the async
//! [`PasskeyStore`] seam, tracking the single-use challenge through the
//! [`ChallengeStore`] seam and verifying the cryptographic ceremony material in
//! [`webauthn`].
//!
//! # Ceremonies
//!
//! * **Registration** — [`PasskeyService::begin_registration`] mints a challenge
//!   and the `PublicKeyCredentialCreationOptions`; the client runs
//!   `navigator.credentials.create`; [`PasskeyService::finish_registration`]
//!   consumes the challenge, verifies the attestation, and persists the public
//!   key bound to the user.
//! * **Authentication** — [`PasskeyService::begin_authentication`] mints a
//!   challenge and request options; the client runs
//!   `navigator.credentials.get`; [`PasskeyService::finish_authentication`]
//!   consumes the challenge, verifies the assertion, checks the signature
//!   counter, and resolves the owning user.
//!
//! # Signature counter
//!
//! The stored counter is advanced only on a **strictly increasing** value. A
//! regression (a cloned authenticator replaying an old assertion) is rejected
//! with [`AuthError::InvalidPasskeyAssertion`]. When both stored and presented
//! counters are zero the authenticator does not support counters, so the check
//! is skipped (mirroring the WebAuthn spec).
//!
//! # Fail closed
//!
//! A missing/consumed challenge, an unknown credential, a mismatched user
//! handle, or a store error all deny the ceremony rather than degrade.

pub mod challenge;
pub mod store;
pub mod types;
pub mod webauthn;

#[cfg(test)]
mod tests;

pub use challenge::{ChallengeStore, DenyAllChallengeStore, MemoryChallengeStore};
pub use store::{DenyAllPasskeyStore, MemoryPasskeyStore, PasskeyCredential, PasskeyStore};
pub use webauthn::{
    generate_challenge, user_handle, verify_assertion, verify_registration, AuthenticationResponse,
    CredentialDescriptor, PubKeyCredParam, PublicKeyCredentialCreationOptions,
    PublicKeyCredentialRequestOptions, RegistrationResponse, RelyingParty, UserEntity, ES256_ALG,
};

use std::sync::Arc;

use chrono::Utc;

use crate::config::ResolvedPasskeys;
use crate::error::{AuthError, Result};

/// Orchestrates the WebAuthn ceremonies over the passkey + challenge seams.
pub struct PasskeyService {
    /// Persistence seam for registered credentials.
    store: Arc<dyn PasskeyStore>,
    /// Single-use challenge-tracking seam.
    challenges: Arc<dyn ChallengeStore>,
    /// WebAuthn relying-party id (the effective domain).
    relying_party_id: String,
    /// Human-readable relying-party name.
    relying_party_name: String,
    /// Origins accepted in `clientDataJSON`.
    allowed_origins: Vec<String>,
    /// HMAC key for user handles.
    user_handle_secret: String,
    /// Ceremony timeout in milliseconds.
    timeout: u64,
}

impl std::fmt::Debug for PasskeyService {
    /// Manual debug — the store and secret are rendered opaquely.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PasskeyService")
            .field("relying_party_id", &self.relying_party_id)
            .field("allowed_origins", &self.allowed_origins)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl PasskeyService {
    /// Build a service over `store`/`challenges` with resolved relying-party settings.
    pub fn new(
        store: Arc<dyn PasskeyStore>,
        challenges: Arc<dyn ChallengeStore>,
        resolved: ResolvedPasskeys,
        relying_party_name: impl Into<String>,
    ) -> Self {
        Self {
            store,
            challenges,
            relying_party_id: resolved.relying_party_id,
            relying_party_name: relying_party_name.into(),
            allowed_origins: resolved.allowed_origins,
            user_handle_secret: resolved.user_handle_secret,
            timeout: resolved.timeout,
        }
    }

    /// Begin a registration ceremony, returning the creation options.
    ///
    /// The challenge is stored under `challenge_key` (the HTTP layer keys it by
    /// the authenticated account) and the user's existing credentials are
    /// excluded so the same authenticator cannot register twice.
    pub async fn begin_registration(
        &self,
        user_id: &str,
        account_name: &str,
        display_name: &str,
        challenge_key: &str,
    ) -> Result<PublicKeyCredentialCreationOptions> {
        let challenge = generate_challenge();
        self.challenges.put(challenge_key, &challenge).await?;
        let existing = self.store.list(user_id).await?;
        let exclude_credentials = existing
            .iter()
            .map(|credential| CredentialDescriptor {
                credential_type: "public-key".to_string(),
                id: credential.id.clone(),
            })
            .collect();
        Ok(PublicKeyCredentialCreationOptions {
            challenge,
            rp: RelyingParty {
                id: self.relying_party_id.clone(),
                name: self.relying_party_name.clone(),
            },
            user: UserEntity {
                id: user_handle(&self.user_handle_secret, user_id)?,
                name: account_name.to_string(),
                display_name: display_name.to_string(),
            },
            pub_key_cred_params: vec![PubKeyCredParam {
                credential_type: "public-key",
                alg: ES256_ALG,
            }],
            timeout: self.timeout,
            attestation: "none",
            exclude_credentials,
        })
    }

    /// Finish a registration ceremony, persisting the new credential.
    ///
    /// Consumes the challenge stored under `challenge_key`; a missing challenge
    /// is [`AuthError::Passkey`] (the ceremony was never started, or was
    /// already completed).
    pub async fn finish_registration(
        &self,
        user_id: &str,
        challenge_key: &str,
        response: &RegistrationResponse,
        name: &str,
    ) -> Result<PasskeyCredential> {
        let expected = self
            .challenges
            .take(challenge_key)
            .await?
            .ok_or_else(|| AuthError::Passkey("no pending registration challenge".into()))?;
        let outcome = verify_registration(
            &self.relying_party_id,
            &self.allowed_origins,
            &expected,
            response,
        )?;
        let credential = PasskeyCredential {
            id: outcome.credential_id,
            user_id: user_id.to_string(),
            public_key: outcome.public_key,
            counter: outcome.counter,
            name: name.to_string(),
            created_at: Utc::now().to_rfc3339(),
        };
        self.store.create(credential.clone()).await?;
        Ok(credential)
    }

    /// Begin an authentication ceremony, returning the request options.
    ///
    /// `allow` narrows the accepted credentials; an empty slice requests a
    /// discoverable-credential (usernameless) ceremony.
    pub async fn begin_authentication(
        &self,
        challenge_key: &str,
        allow: &[PasskeyCredential],
    ) -> Result<PublicKeyCredentialRequestOptions> {
        let challenge = generate_challenge();
        self.challenges.put(challenge_key, &challenge).await?;
        let allow_credentials = allow
            .iter()
            .map(|credential| CredentialDescriptor {
                credential_type: "public-key".to_string(),
                id: credential.id.clone(),
            })
            .collect();
        Ok(PublicKeyCredentialRequestOptions {
            challenge,
            rp_id: self.relying_party_id.clone(),
            timeout: self.timeout,
            allow_credentials,
            user_verification: "preferred",
        })
    }

    /// Finish an authentication ceremony, returning the authenticated credential.
    ///
    /// Consumes the challenge, resolves the credential by id, verifies the
    /// assertion and the user handle binding, then enforces the signature
    /// counter before advancing it. The caller resolves the owning user from
    /// [`PasskeyCredential::user_id`].
    pub async fn finish_authentication(
        &self,
        challenge_key: &str,
        response: &AuthenticationResponse,
    ) -> Result<PasskeyCredential> {
        let expected = self
            .challenges
            .take(challenge_key)
            .await?
            .ok_or_else(|| AuthError::Passkey("no pending authentication challenge".into()))?;
        let credential = self
            .store
            .find(&response.id)
            .await?
            .ok_or(AuthError::PasskeyRequired)?;
        let outcome = verify_assertion(
            &self.relying_party_id,
            &self.allowed_origins,
            &expected,
            &credential.public_key,
            response,
        )?;
        if let Some(handle) = outcome.user_handle.as_deref() {
            if handle != user_handle(&self.user_handle_secret, &credential.user_id)? {
                return Err(AuthError::InvalidPasskeyAssertion);
            }
        }
        if (credential.counter != 0 || outcome.counter != 0)
            && outcome.counter <= credential.counter
        {
            return Err(AuthError::InvalidPasskeyAssertion);
        }
        if outcome.counter > credential.counter {
            self.store
                .update_counter(&credential.id, outcome.counter)
                .await?;
        }
        Ok(credential)
    }

    /// List the credentials owned by `user_id`.
    pub async fn list_credentials(&self, user_id: &str) -> Result<Vec<PasskeyCredential>> {
        self.store.list(user_id).await
    }

    /// Delete `credential_id` if it is owned by `user_id`.
    pub async fn delete_credential(&self, user_id: &str, credential_id: &str) -> Result<()> {
        self.store.delete(user_id, credential_id).await
    }
}
