/// Broadcast error type for the realtime layer.
use thiserror::Error;

/// Alias for results produced by broadcast operations.
pub type Result<T> = std::result::Result<T, BroadcastError>;

/// Top-level broadcast error type.
#[derive(Debug, Error)]
pub enum BroadcastError {
    /// The caller is not allowed on a private/presence channel
    /// (HTTP 403 / WebSocket close 4403).
    #[error("unauthorized on channel {channel}")]
    Unauthorized {
        /// Channel the caller attempted to join.
        channel: String,
    },

    /// A channel requires authentication but no identity was supplied.
    #[error("channel {channel} requires authentication")]
    Unauthenticated {
        /// Channel that demanded an identity.
        channel: String,
    },

    /// The caller is not allowed on a private/presence channel.
    #[error("forbidden on channel {channel}: {reason}")]
    Forbidden {
        /// Channel the caller attempted to join.
        channel: String,
        /// Authorization denial reason.
        reason: String,
    },

    /// The underlying transport (WebSocket/SSE) failed.
    #[error("broadcast transport failed: {0}")]
    Transport(String),

    /// A serialization failure occurred while encoding a payload.
    #[error("broadcast serialization failed: {0}")]
    Serialization(String),

    /// The selected broadcast connection is declared but not fully configured
    /// (missing credentials or a required field), so it cannot be built.
    ///
    /// Surfaces instead of silently falling back to the in-process hub, so a
    /// production config can never quietly degrade (ADOPT-022).
    #[error("broadcast connection {connection} is not configured")]
    NotConfigured {
        /// Connection name that is missing required configuration.
        connection: String,
    },

    /// A connection name was requested that the manager does not know.
    #[error("unknown broadcast connection {connection}")]
    ConnectionUnknown {
        /// Connection name that was requested.
        connection: String,
    },

    /// An external driver failed (HTTP transport, a non-2xx status, or a
    /// Pub/Sub transport error). Carries the connection name and a message that
    /// includes the status code plus a response-body snippet where available.
    #[error("broadcast driver {connection} failed: {message}")]
    Driver {
        /// Connection name whose driver failed.
        connection: String,
        /// Human-readable failure detail (status code + body snippet).
        message: String,
    },
}

impl From<serde_json::Error> for BroadcastError {
    /// Convert a JSON encoding failure into a typed broadcast error.
    fn from(e: serde_json::Error) -> Self {
        BroadcastError::Serialization(e.to_string())
    }
}
