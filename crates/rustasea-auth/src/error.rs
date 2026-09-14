/// Typed errors for the rustasea-auth crate.
///
/// Every variant carries a stable `code` string, a user-facing `hint`, and
/// preserves the underlying cause so callers can map to JSON error envelopes.
use thiserror::Error;

/// Alias for results produced by auth operations.
pub type Result<T> = std::result::Result<T, AuthError>;

/// Top-level authentication error type.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthError {
    /// No guard is registered under the requested name.
    ///
    /// `expected` names the guard the manager knows (typically the default),
    /// `actual` the name the caller asked for.
    #[error("guard mismatch: expected {expected:?}, actual {actual:?}")]
    GuardMismatch {
        /// Guard name that is registered (the manager default).
        expected: String,
        /// Guard name that was requested.
        actual: String,
    },

    /// Login credentials do not match any known user / password.
    #[error("bad credentials")]
    BadCredentials,

    /// Supplied bearer token could not be decoded or verified.
    #[error("invalid token")]
    InvalidToken,

    /// Supplied bearer token is well-formed but its `exp` claim is in the past.
    #[error("token expired")]
    ExpiredToken,

    /// Guard is disabled by configuration (e.g. `loginUsingId` in production).
    #[error("guard operation disabled: {0}")]
    Disabled(String),

    /// Underlying JWT / crypto failure with a stable code.
    #[error("token error: {0}")]
    Token(String),

    /// Password hash could not be verified (malformed stored hash).
    #[error("hash error: {0}")]
    Hash(String),

    /// Session store read/write failure (Redis/DB down).
    #[error("session store unavailable")]
    StoreUnavailable,

    /// A user with the same unique email already exists (registration).
    #[error("user already exists for email {email}")]
    UserExists {
        /// Email that collided with an existing user.
        email: String,
    },

    /// No user matches the requested identifier (password reset / update).
    #[error("user not found: {id}")]
    UserNotFound {
        /// Identifier (UUID) that resolved to no user.
        id: String,
    },

    /// A login was interrupted because the account has confirmed two-factor
    /// authentication, and the challenge has not been completed.
    #[error("two-factor authentication required")]
    TwoFactorRequired,

    /// The supplied TOTP code or recovery code did not verify.
    #[error("invalid two-factor code")]
    InvalidTwoFactorCode,

    /// Two-factor secret handling failed (crypto, sealing, or key config).
    #[error("two-factor error: {0}")]
    TwoFactor(String),

    /// The account has no usable passkey, or the presented credential is
    /// unknown — the ceremony cannot proceed.
    #[error("passkey authentication required")]
    PasskeyRequired,

    /// A WebAuthn assertion failed verification (signature, origin, challenge,
    /// or signature-counter regression).
    #[error("invalid passkey assertion")]
    InvalidPasskeyAssertion,

    /// Passkey ceremony handling failed (malformed input, CBOR/COSE decoding,
    /// unsupported attestation, or credential persistence).
    #[error("passkey error: {0}")]
    Passkey(String),
}

impl AuthError {
    /// Stable machine-readable code, e.g. `AuthError::GuardMismatch`.
    pub fn code(&self) -> String {
        let variant = match self {
            AuthError::GuardMismatch { .. } => "GuardMismatch",
            AuthError::BadCredentials => "BadCredentials",
            AuthError::InvalidToken => "InvalidToken",
            AuthError::ExpiredToken => "ExpiredToken",
            AuthError::Disabled(_) => "Disabled",
            AuthError::Token(_) => "Token",
            AuthError::Hash(_) => "Hash",
            AuthError::StoreUnavailable => "StoreUnavailable",
            AuthError::UserExists { .. } => "UserExists",
            AuthError::UserNotFound { .. } => "UserNotFound",
            AuthError::TwoFactorRequired => "TwoFactorRequired",
            AuthError::InvalidTwoFactorCode => "InvalidTwoFactorCode",
            AuthError::TwoFactor(_) => "TwoFactor",
            AuthError::PasskeyRequired => "PasskeyRequired",
            AuthError::InvalidPasskeyAssertion => "InvalidPasskeyAssertion",
            AuthError::Passkey(_) => "Passkey",
        };
        format!("AuthError::{variant}")
    }

    /// Short user-facing remediation hint.
    pub fn hint(&self) -> &'static str {
        match self {
            AuthError::GuardMismatch { .. } => {
                "Request a guard name that is registered (e.g. jwt, session)."
            }
            AuthError::BadCredentials => "Verify the email/password combination.",
            AuthError::InvalidToken => "Re-authenticate to obtain a fresh token.",
            AuthError::ExpiredToken => "Refresh the token before it expires.",
            AuthError::Disabled(_) => "This guard operation is disabled by configuration.",
            AuthError::Token(_) => "The token could not be processed; obtain a new one.",
            AuthError::Hash(_) => "Re-hash the stored password.",
            AuthError::StoreUnavailable => "The session store is unreachable; retry later.",
            AuthError::UserExists { .. } => {
                "Choose a different email address; this one is already registered."
            }
            AuthError::UserNotFound { .. } => "Re-check the user identifier.",
            AuthError::TwoFactorRequired => "Complete the two-factor challenge to continue.",
            AuthError::InvalidTwoFactorCode => "Enter a valid authenticator or recovery code.",
            AuthError::TwoFactor(_) => "Check the two-factor key configuration and retry.",
            AuthError::PasskeyRequired => "Register a passkey or use another sign-in method.",
            AuthError::InvalidPasskeyAssertion => "Use a registered passkey for this site.",
            AuthError::Passkey(_) => "Check the passkey relying-party configuration and retry.",
        }
    }
}

/// Typed configuration errors raised while parsing `auth.toml` / `session.toml`.
///
/// These surface at boot (before any request is served), so a misconfigured
/// session driver or cookie policy fails fast instead of degrading silently.
/// Each variant carries a stable `code` via [`AuthConfigError::code`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthConfigError {
    /// The `[auth]` / `[session]` table exists but cannot be deserialized.
    #[error("invalid auth configuration: {0}")]
    Invalid(String),

    /// A required table or key is absent from the configuration.
    #[error("missing auth configuration: {0}")]
    Missing(String),

    /// `session.same_site` was not one of `lax`, `strict`, or `none`.
    #[error("invalid session same_site {0:?} (expected \"lax\", \"strict\", or \"none\")")]
    InvalidSameSite(String),

    /// `session.serialization` was not `json`.
    #[error("unsupported session serialization {0:?} (only \"json\" is allowed)")]
    UnsupportedSerialization(String),

    /// `session.driver` names a store that is not implemented yet.
    #[error("unsupported session driver {0:?} (only \"memory\" and \"database\" are implemented)")]
    UnsupportedSessionDriver(String),

    /// The configured database session store could not be opened.
    ///
    /// Raised while wiring `driver = "database"`: the named connection could not
    /// be resolved or reached. The message carries the underlying cause.
    #[error("session store unavailable: {0}")]
    SessionStoreUnavailable(String),

    /// The named guard is not declared in `[auth.guards]`.
    #[error("unknown auth guard {0:?}")]
    UnknownGuard(String),

    /// The guard declares no `provider`, or the named provider is undeclared.
    #[error("guard {guard:?} has no usable provider {provider:?}")]
    MissingGuardProvider {
        /// Guard whose provider could not be resolved.
        guard: String,
        /// Provider name the guard referenced (possibly empty).
        provider: String,
    },
}

impl AuthConfigError {
    /// Stable machine-readable code, e.g. `AuthConfigError::InvalidSameSite`.
    pub fn code(&self) -> String {
        let variant = match self {
            AuthConfigError::Invalid(_) => "Invalid",
            AuthConfigError::Missing(_) => "Missing",
            AuthConfigError::InvalidSameSite(_) => "InvalidSameSite",
            AuthConfigError::UnsupportedSerialization(_) => "UnsupportedSerialization",
            AuthConfigError::UnsupportedSessionDriver(_) => "UnsupportedSessionDriver",
            AuthConfigError::SessionStoreUnavailable(_) => "SessionStoreUnavailable",
            AuthConfigError::UnknownGuard(_) => "UnknownGuard",
            AuthConfigError::MissingGuardProvider { .. } => "MissingGuardProvider",
        };
        format!("AuthConfigError::{variant}")
    }

    /// Short user-facing remediation hint.
    pub fn hint(&self) -> &'static str {
        match self {
            AuthConfigError::Invalid(_) | AuthConfigError::Missing(_) => {
                "Fix the auth/session config file to match the documented shape."
            }
            AuthConfigError::InvalidSameSite(_) => {
                "Set session.same_site to \"lax\", \"strict\", or \"none\"."
            }
            AuthConfigError::UnsupportedSerialization(_) => {
                "Set session.serialization to \"json\"."
            }
            AuthConfigError::UnsupportedSessionDriver(_) => {
                "Set session.driver to \"memory\" or \"database\"."
            }
            AuthConfigError::SessionStoreUnavailable(_) => {
                "Check the session.connection name and that the database is reachable."
            }
            AuthConfigError::UnknownGuard(_) => {
                "Declare the guard under [auth.guards.<name>] or fix auth.defaults.guard."
            }
            AuthConfigError::MissingGuardProvider { .. } => {
                "Declare the guard's provider under [auth.providers.<name>]."
            }
        }
    }
}

/// Typed CSRF (forgery-protection) errors.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CsrfError {
    /// Request is missing or carries an invalid `X-CSRF-TOKEN`/`_token`.
    #[error("csrf token mismatch")]
    TokenMismatch,

    /// `Sec-Fetch-Site: cross-site` with an origin outside the allow-list.
    #[error("origin {origin} is not in the csrf allow-list")]
    UntrustedOrigin {
        /// The offending Origin header value.
        origin: String,
    },
}

impl CsrfError {
    /// Stable machine-readable code, e.g. `CsrfError::UntrustedOrigin`.
    pub fn code(&self) -> String {
        let variant = match self {
            CsrfError::TokenMismatch => "TokenMismatch",
            CsrfError::UntrustedOrigin { .. } => "UntrustedOrigin",
        };
        format!("CsrfError::{variant}")
    }
}

/// Typed session/cache serialization-policy errors (hardened deserialization).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SerializationError {
    /// Attempted to deserialize a type outside `serializable_classes`.
    #[error("type {type_name} is not in the serializable_classes allow-list")]
    NotAllowed {
        /// Type name that was rejected.
        type_name: String,
    },
}

/// Typed throttle errors surfaced by the rate limiter.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ThrottleError {
    /// A custom key function returned no key (request identity unknown).
    #[error("throttle key unavailable: {0}")]
    KeyUnavailable(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Codes follow the `AuthError::Variant` wire contract (api-auth.md §4).
    #[test]
    fn error_codes_match_wire_contract() {
        assert_eq!(
            AuthError::BadCredentials.code(),
            "AuthError::BadCredentials"
        );
        assert_eq!(
            AuthError::GuardMismatch {
                expected: "jwt".into(),
                actual: "api".into()
            }
            .code(),
            "AuthError::GuardMismatch"
        );
        assert_eq!(
            CsrfError::UntrustedOrigin {
                origin: "https://evil.com".into()
            }
            .code(),
            "CsrfError::UntrustedOrigin"
        );
        assert_eq!(
            AuthError::UserExists {
                email: "ada@example.com".into()
            }
            .code(),
            "AuthError::UserExists"
        );
        assert_eq!(
            AuthError::UserNotFound {
                id: "user-1".into()
            }
            .code(),
            "AuthError::UserNotFound"
        );
    }

    /// Config-error codes follow the same `Type::Variant` wire contract.
    #[test]
    fn config_error_codes_match_wire_contract() {
        assert_eq!(
            AuthConfigError::InvalidSameSite("weird".into()).code(),
            "AuthConfigError::InvalidSameSite"
        );
        assert_eq!(
            AuthConfigError::UnsupportedSerialization("msgpack".into()).code(),
            "AuthConfigError::UnsupportedSerialization"
        );
        assert_eq!(
            AuthConfigError::MissingGuardProvider {
                guard: "web".into(),
                provider: String::new(),
            }
            .code(),
            "AuthConfigError::MissingGuardProvider"
        );
    }
}
