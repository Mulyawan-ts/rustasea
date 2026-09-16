//! RustaSea Google — service-account auth with cached access tokens.
//!
//! Parity target: `google/auth` — the `ServiceAccountCredentials` flow. A
//! service account is parsed from its JSON key, a short-lived RS256 JWT
//! **assertion** is signed with the account's private key, and the assertion is
//! exchanged at the account's token endpoint for a bearer **access token**.
//! Tokens are cached in-process and refreshed once 80% of their lifetime has
//! elapsed, so a caller can request a token on every outbound call without
//! paying an HTTP round-trip each time. Concurrent callers share one refresh
//! (single-flight).
//!
//! ## Modules
//!
//! * [`credentials`] — service-account JSON parsing, validation, and a `Debug`
//!   implementation that redacts the private key.
//! * [`claims`] — the signed JWT assertion claim set
//!   (`iss`/`sub`/`aud`/`exp`/`iat`/`scope`).
//! * [`token`] — the cached [`AccessToken`] with expiry + refresh-threshold math.
//! * [`client`] — [`GoogleAuthClient`], the token cache with single-flight
//!   refresh, and the swappable [`TokenTransport`]/[`Clock`] seams.
//! * [`error`] — the typed [`GoogleAuthError`].
//!
//! ## Example
//!
//! ```no_run
//! use rustasea_google::{GoogleAuthClient, ServiceAccount};
//!
//! # async fn run() -> Result<(), rustasea_google::GoogleAuthError> {
//! let account = ServiceAccount::from_file("/etc/credentials.json")?;
//! let client = GoogleAuthClient::new(
//!     account,
//!     vec!["https://www.googleapis.com/auth/cloud-platform".to_string()],
//!     None,
//! )?;
//! let token = client.token().await?;
//! // Send `token.value()` as the `Authorization: Bearer …` credential.
//! # Ok(())
//! # }
//! ```

pub mod claims;
pub mod client;
pub mod credentials;
pub mod error;
pub mod token;

#[cfg(test)]
pub(crate) mod test_support;

pub use claims::AssertionClaims;
pub use client::{
    Clock, GoogleAuthClient, GoogleTokenSource, ReqwestTransport, SystemClock, TokenTransport,
    DEFAULT_REFRESH_RATIO, DEFAULT_TIMEOUT_SECS,
};
pub use credentials::ServiceAccount;
pub use error::{GoogleAuthError, Result};
pub use token::AccessToken;
