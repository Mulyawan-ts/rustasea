/// Typed errors for the queue layer.
use std::sync::PoisonError;

use thiserror::Error;

/// Alias for results produced by queue operations.
pub type Result<T> = std::result::Result<T, QueueError>;

/// Top-level queue error type.
///
/// Mirrors the `QueueError`/`JobError` catalogue from api-queue.md §4: a
/// duplicated boot-time route is a `DuplicateRoute`, an unknown connection
/// name is `UnknownConnection`, and an unreachable backing store surfaces as
/// `StoreUnavailable` — never a panic.
#[derive(Debug, Error)]
pub enum QueueError {
    /// A second `Queue::route::<J>` was attempted for an already-registered type.
    #[error("duplicate queue route for {type_name}: already routed")]
    DuplicateRoute {
        /// Fully-qualified job type name.
        type_name: &'static str,
    },

    /// No driver is registered under the requested connection name.
    #[error("unknown queue connection: {0}")]
    UnknownConnection(String),

    /// The backing store (Redis/DB) is unreachable.
    #[error("queue store unavailable: {0}")]
    StoreUnavailable(String),

    /// A job could not be serialized into its queue payload.
    #[error("job serialization failed: {0}")]
    Serialization(String),

    /// The routed queue registry is not yet booted (no route registered).
    #[error("no route registered for job type {0}; register with Queue::route before dispatch")]
    Unrouted(String),

    /// The registry lock was poisoned by a panicking dispatcher.
    #[error("queue registry lock poisoned")]
    RegistryPoisoned,

    /// No queued job was available for reservation.
    #[error("queue {0} is empty")]
    Empty(String),

    /// A `[queue]` configuration table could not be parsed or validated.
    ///
    /// Wraps a structured [`QueueConfigError`] message so registry wiring that
    /// returns a [`Result`] surfaces config problems without a second error type.
    #[error("queue configuration error: {0}")]
    Config(String),
}

/// Typed `[queue]` configuration errors (Laravel `config/queue.php` parity).
///
/// Produced by [`crate::config::QueueConfig`] parsing/validation and by
/// [`crate::wiring::register_from_config`]. Every variant is a load-time error,
/// so a misconfigured queue connection fails fast rather than silently falling
/// back to the `sync` driver in production.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum QueueConfigError {
    /// The `[queue]` table exists but could not be deserialized.
    #[error("invalid queue config: {0}")]
    Invalid(String),

    /// `queue.default` (or a route) names a connection not declared under
    /// `[queue.connections]`.
    #[error("unknown queue connection `{name}`")]
    UnknownConnection {
        /// The undeclared connection name.
        name: String,
    },

    /// A connection declares a driver RustaSea does not implement.
    ///
    /// Covers both genuinely unknown names and the recognised-but-unimplemented
    /// Laravel drivers (`beanstalkd`, `sqs`, `deferred`, `background`,
    /// `failover`); the offending driver is always named.
    #[error("unsupported queue driver `{driver}` for connection `{connection}`")]
    UnsupportedDriver {
        /// Connection declaring the unsupported driver.
        connection: String,
        /// The unsupported driver name.
        driver: String,
    },

    /// A connection is missing a field its driver requires (e.g. `table` for
    /// the `database` driver).
    #[error("queue connection `{connection}` is missing required field `{field}`")]
    MissingField {
        /// Connection missing the field.
        connection: String,
        /// Name of the missing field.
        field: String,
    },
}

impl From<QueueConfigError> for QueueError {
    /// Map a typed config error onto the top-level [`QueueError::Config`].
    fn from(error: QueueConfigError) -> Self {
        QueueError::Config(error.to_string())
    }
}

impl From<PoisonError<std::sync::RwLockWriteGuard<'_, crate::registry::RegistryInner>>>
    for QueueError
{
    /// Convert a poisoned registry write lock into a typed error.
    fn from(
        _: PoisonError<std::sync::RwLockWriteGuard<'_, crate::registry::RegistryInner>>,
    ) -> Self {
        QueueError::RegistryPoisoned
    }
}

impl From<PoisonError<std::sync::RwLockReadGuard<'_, crate::registry::RegistryInner>>>
    for QueueError
{
    /// Convert a poisoned registry read lock into a typed error.
    fn from(
        _: PoisonError<std::sync::RwLockReadGuard<'_, crate::registry::RegistryInner>>,
    ) -> Self {
        QueueError::RegistryPoisoned
    }
}

impl From<rustasea_orm::OrmError> for QueueError {
    /// Map a backing-store failure onto the queue's `StoreUnavailable`.
    fn from(error: rustasea_orm::OrmError) -> Self {
        QueueError::StoreUnavailable(error.to_string())
    }
}

/// Job-execution error returned by `Job::handle`.
///
/// `Timeout`/`MaxAttemptsExceeded` are produced by the worker loop; user
/// handlers return `Exception` for domain failures.
#[derive(Debug, Error)]
pub enum JobError {
    /// A configured per-job timeout elapsed before `handle` returned.
    #[error("job timed out")]
    Timeout,

    /// The job exhausted its retry budget.
    #[error("max attempts exceeded")]
    MaxAttemptsExceeded,

    /// A domain failure reported by `Job::handle`.
    #[error("{0}")]
    Exception(String),
}
