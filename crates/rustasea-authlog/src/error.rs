//! Typed errors for the authentication-log crate.

use thiserror::Error;

/// Alias for results produced by the authentication-log repository.
pub type Result<T> = std::result::Result<T, AuthLogError>;

/// Errors raised while recording or querying the authentication log.
#[derive(Debug, Error)]
pub enum AuthLogError {
    /// The underlying ORM/storage call failed.
    #[error("authentication log storage error: {0}")]
    Storage(String),

    /// A query was issued with an invalid argument.
    #[error("authentication log invalid argument: {0}")]
    InvalidArgument(String),

    /// A new-device notification could not be queued.
    #[error("authentication log mail error: {0}")]
    Mail(String),
}

impl AuthLogError {
    /// Stable machine-readable error code for this variant.
    ///
    /// These strings are part of the crate's contract (HTTP problem codes,
    /// logs) and must not change once released.
    pub fn code(&self) -> &'static str {
        match self {
            AuthLogError::Storage(_) => "AuthLogError::Storage",
            AuthLogError::InvalidArgument(_) => "AuthLogError::InvalidArgument",
            AuthLogError::Mail(_) => "AuthLogError::Mail",
        }
    }
}

impl From<rustasea_orm::OrmError> for AuthLogError {
    /// Map an ORM failure onto the authentication-log storage error.
    fn from(error: rustasea_orm::OrmError) -> Self {
        AuthLogError::Storage(error.to_string())
    }
}

impl From<rustasea_mail::MailError> for AuthLogError {
    /// Map a mail failure onto the authentication-log mail error.
    fn from(error: rustasea_mail::MailError) -> Self {
        AuthLogError::Mail(error.to_string())
    }
}
