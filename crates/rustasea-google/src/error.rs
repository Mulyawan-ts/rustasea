//! Typed errors for service-account auth (ADOPT-026).
//!
//! Every variant carries only a human-readable `message` that is safe to log:
//! the private key material and raw access-token values are never included.
//! Transport/endpoint failures name the HTTP status and, at most, the OAuth
//! `error`/`error_description` fields — never the raw response body.

use thiserror::Error;

/// Result alias for Google auth operations.
pub type Result<T> = std::result::Result<T, GoogleAuthError>;

/// Errors raised while loading credentials, signing an assertion, or
/// exchanging it for an access token.
#[derive(Debug, Error)]
pub enum GoogleAuthError {
    /// The service-account JSON file could not be read from disk.
    #[error("failed to read google service-account file: {message}")]
    CredentialsFile {
        /// Filesystem error detail (includes the path, never the file body).
        message: String,
    },

    /// The service-account JSON is malformed or is missing a required field.
    #[error("invalid google service-account credentials: {message}")]
    InvalidCredentials {
        /// Which field or JSON fragment failed validation.
        message: String,
    },

    /// The `private_key` value is not a usable PEM-encoded RSA key.
    #[error("invalid google service-account private key: {message}")]
    InvalidPrivateKey {
        /// Key-parsing error detail (never the key bytes).
        message: String,
    },

    /// Signing the RS256 assertion failed.
    #[error("failed to sign google assertion: {message}")]
    Signing {
        /// Signing error detail (never the key bytes).
        message: String,
    },

    /// No OAuth scopes were configured, so no assertion can be scoped.
    #[error("google auth requires at least one scope")]
    MissingScopes,

    /// The HTTP exchange with the token endpoint failed at the transport layer.
    #[error("google token endpoint transport failed: {message}")]
    Transport {
        /// Transport error detail (DNS, connection, timeout).
        message: String,
    },

    /// The token endpoint returned a non-success status.
    #[error("google token endpoint returned status {status}: {message}")]
    TokenEndpoint {
        /// HTTP status code returned by the endpoint.
        status: u16,
        /// OAuth `error`/`error_description`, or a generic fallback.
        message: String,
    },

    /// The token endpoint returned a success status but an unusable body.
    #[error("google token endpoint returned an invalid response: {message}")]
    TokenResponse {
        /// Decoding error detail (never the response body).
        message: String,
    },
}
