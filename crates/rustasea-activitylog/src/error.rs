//! Typed errors for the activity-log crate.

use thiserror::Error;

/// Alias for results produced by the activity-log repository.
pub type Result<T> = std::result::Result<T, ActivityError>;

/// Errors raised while recording or querying the audit log.
#[derive(Debug, Error)]
pub enum ActivityError {
    /// The underlying ORM/storage call failed.
    #[error("activity log storage error: {0}")]
    Storage(String),

    /// A stored row could not be decoded into an [`crate::Activity`].
    #[error("activity log decode error: {0}")]
    Decode(String),

    /// A query was issued with an invalid argument (e.g. a non-UUID subject id).
    #[error("activity log invalid argument: {0}")]
    InvalidArgument(String),
}

impl From<rustasea_orm::OrmError> for ActivityError {
    /// Map an ORM failure onto the activity-log storage error.
    fn from(error: rustasea_orm::OrmError) -> Self {
        ActivityError::Storage(error.to_string())
    }
}
