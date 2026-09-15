//! Typed errors for the action pattern.
//!
//! Every adapter maps an [`ActionError`] onto its own transport: HTTP renders
//! the 422 body from a [`rustasea_validation::ErrorBag`], the queue dead-letters
//! the message, the CLI prints it, and the event layer wraps it in an
//! `EventError`. Keeping one error type means an action's failure surface is
//! identical no matter where it runs.

use rustasea_validation::ErrorBag;
use thiserror::Error;

/// Alias for results produced by action adapters.
pub type Result<T> = std::result::Result<T, ActionError>;

/// Errors produced while invoking an action through any adapter.
#[derive(Debug, Error)]
pub enum ActionError {
    /// An input could not be deserialized (bad CLI JSON, malformed payload).
    #[error("action input serialization failed: {0}")]
    Serialization(String),

    /// The action's `validate` hook rejected the input.
    ///
    /// Carries the field-keyed bag so the HTTP adapter can render the
    /// documented 422 body verbatim.
    #[error("action input failed validation")]
    Validation(ErrorBag),

    /// The action's `authorize` hook returned `false`.
    #[error("action is not authorized for this input")]
    Unauthorized,

    /// The action's `handle` body failed with a domain error.
    #[error("{0}")]
    Failed(String),
}

impl ActionError {
    /// Whether this error is a validation failure (HTTP 422).
    pub fn is_validation(&self) -> bool {
        matches!(self, ActionError::Validation(_))
    }

    /// Whether this error is an authorization failure (HTTP 403).
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, ActionError::Unauthorized)
    }
}

impl From<ErrorBag> for ActionError {
    /// A rejected validation bag becomes [`ActionError::Validation`].
    fn from(bag: ErrorBag) -> Self {
        ActionError::Validation(bag)
    }
}

impl From<serde_json::Error> for ActionError {
    /// A serde failure becomes [`ActionError::Serialization`].
    fn from(error: serde_json::Error) -> Self {
        ActionError::Serialization(error.to_string())
    }
}
