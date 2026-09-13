//! Typed errors for the logging facade.
//!
//! Every failure mode is expressed as a [`LoggingError`] variant so callers
//! match on stable types instead of strings. Configuration problems, unknown
//! or unsupported drivers, missing channels and subscriber-install failures all
//! surface here; the crate never panics on malformed input.

use thiserror::Error;

/// Alias for results produced by the logging facade.
pub type Result<T> = std::result::Result<T, LoggingError>;

/// Top-level error type for the logging layer.
#[derive(Debug, Error)]
pub enum LoggingError {
    /// The `[logging]` table exists but is malformed (bad TOML, invalid level,
    /// empty stack, …).
    #[error("logging configuration invalid: {0}")]
    InvalidConfig(String),

    /// A channel declares a driver that this crate does not recognise.
    #[error("unknown logging driver `{0}`")]
    UnknownDriver(String),

    /// A channel declares a driver that is recognised but not implemented
    /// (`slack`, `papertrail`, `syslog`).
    #[error("logging driver `{0}` is not supported by rustasea-logging")]
    UnsupportedDriver(String),

    /// The selected channel (or a stack member) is not defined.
    #[error("logging channel `{0}` is not defined")]
    UnknownChannel(String),

    /// A `stack` channel references itself, directly or transitively.
    #[error("logging stack `{0}` references itself")]
    CyclicStack(String),

    /// A file appender could not be constructed (bad path, permissions, …).
    #[error("logging io error: {0}")]
    Io(String),

    /// The global subscriber could not be installed.
    #[error("failed to initialise the global subscriber: {0}")]
    Init(String),
}

impl LoggingError {
    /// True when the error describes an unrecognised driver.
    pub fn is_unknown_driver(&self) -> bool {
        matches!(self, LoggingError::UnknownDriver(_))
    }

    /// True when the error describes a recognised-but-unsupported driver.
    pub fn is_unsupported_driver(&self) -> bool {
        matches!(self, LoggingError::UnsupportedDriver(_))
    }
}
