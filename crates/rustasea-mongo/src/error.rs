//! Typed errors for the MongoDB document store.
//!
//! Driver failures are funnelled through [`map_driver_error`] so callers only
//! ever match on the four [`MongoError`] variants instead of the driver's
//! `#[non_exhaustive]` error kind.

use thiserror::Error;

/// Alias for results produced by Mongo operations.
pub type Result<T> = std::result::Result<T, MongoError>;

/// Top-level error type for the Mongo layer.
///
/// Every driver error is classified into one of these variants by
/// [`map_driver_error`], keeping application matches stable across driver
/// releases.
#[derive(Debug, Error)]
pub enum MongoError {
    /// The cluster is unreachable, authentication failed, or the connection
    /// pool was cleared mid-operation.
    #[error("mongo connection failed: {0}")]
    Connection(String),

    /// The configuration is malformed (bad URI, empty database, missing env).
    #[error("mongo configuration invalid: {0}")]
    Configuration(String),

    /// A value could not be encoded to / decoded from BSON.
    #[error("mongo serialization failed: {0}")]
    Serialization(String),

    /// The server rejected an otherwise valid operation.
    #[error("mongo operation failed: {0}")]
    Operation(String),
}

impl MongoError {
    /// True when the error describes an unreachable or unusable cluster.
    pub fn is_connection(&self) -> bool {
        matches!(self, MongoError::Connection(_))
    }

    /// True when the error describes malformed configuration.
    pub fn is_configuration(&self) -> bool {
        matches!(self, MongoError::Configuration(_))
    }
}

/// Classify a driver error into a [`MongoError`].
///
/// Invalid arguments map to [`MongoError::Configuration`]; DNS, TLS,
/// authentication, server-selection, I/O and pool failures map to
/// [`MongoError::Connection`]; BSON encode/decode failures map to
/// [`MongoError::Serialization`]; everything else maps to
/// [`MongoError::Operation`].
pub fn map_driver_error(err: mongodb::error::Error) -> MongoError {
    use mongodb::error::ErrorKind;

    let message = err.to_string();
    match err.kind.as_ref() {
        ErrorKind::InvalidArgument { .. } => MongoError::Configuration(message),
        ErrorKind::Authentication { .. }
        | ErrorKind::ServerSelection { .. }
        | ErrorKind::Io(_)
        | ErrorKind::ConnectionPoolCleared { .. }
        | ErrorKind::DnsResolve { .. }
        | ErrorKind::Shutdown => MongoError::Connection(message),
        ErrorKind::BsonSerialization(_)
        | ErrorKind::BsonDeserialization(_)
        | ErrorKind::Bson(_) => MongoError::Serialization(message),
        _ => MongoError::Operation(message),
    }
}

impl From<mongodb::error::Error> for MongoError {
    /// Convenience conversion mirroring [`map_driver_error`].
    fn from(err: mongodb::error::Error) -> Self {
        map_driver_error(err)
    }
}

impl From<bson::error::Error> for MongoError {
    /// Map a bare BSON error (e.g. from the builders) to serialization.
    fn from(err: bson::error::Error) -> Self {
        MongoError::Serialization(err.to_string())
    }
}
