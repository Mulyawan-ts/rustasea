//! Typed errors for the timezone layer.

use thiserror::Error;

/// Alias for results produced by timezone operations.
pub type Result<T> = std::result::Result<T, TimezoneError>;

/// Failures raised while validating or resolving a timezone name.
///
/// A timezone name that does not parse as an IANA identifier (nor match a
/// documented short alias) is rejected with [`TimezoneError::InvalidTimezone`]
/// — the resolver never panics, and never silently substitutes a wrong zone.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TimezoneError {
    /// A timezone name failed IANA validation.
    #[error("invalid timezone `{name}`: {reason}")]
    InvalidTimezone {
        /// The rejected name (as supplied, before trimming).
        name: String,
        /// Human-readable reason for the rejection.
        reason: String,
    },
}

impl TimezoneError {
    /// Stable machine-readable variant name, e.g. `TimezoneError::InvalidTimezone`.
    ///
    /// Intended for structured logging and assertions that should not depend on
    /// the human-readable message.
    pub fn code(&self) -> &'static str {
        match self {
            TimezoneError::InvalidTimezone { .. } => "TimezoneError::InvalidTimezone",
        }
    }
}
