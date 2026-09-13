//! Mail error types.

use thiserror::Error;

use rustasea_queue::QueueError;

/// Alias for results produced by mail operations.
pub type Result<T> = std::result::Result<T, MailError>;

/// Top-level mail error type.
#[derive(Debug, Error)]
pub enum MailError {
    /// The transport failed to deliver the message.
    #[error("mail transport failed: {0}")]
    Transport(String),

    /// An address is malformed or a required recipient is missing.
    #[error("invalid mail recipient: {0}")]
    InvalidRecipient(String),

    /// A message could not be assembled from its parts.
    #[error("mail message could not be built: {0}")]
    Render(String),

    /// A mailer could not be built from configuration.
    #[error(transparent)]
    Config(#[from] MailConfigError),

    /// Enqueueing a queued notification failed.
    #[error(transparent)]
    Queue(#[from] QueueError),
}

/// Errors raised while loading or applying `[mail]` configuration.
///
/// These are separated from [`MailError`] so configuration can be validated
/// (and the error matched) without pulling in a transport. [`MailError`] wraps
/// this type via its [`MailError::Config`] variant, so `?` lifts it into the
/// crate-wide [`Result`].
#[derive(Debug, Error)]
pub enum MailConfigError {
    /// The `[mail]` table exists but cannot be deserialized.
    #[error("mail configuration invalid: {0}")]
    InvalidConfig(String),

    /// The `default` selector names a mailer that is not defined.
    #[error("default mailer `{0}` is not defined in [mail.mailers]")]
    UnknownDefaultMailer(String),

    /// A mailer names a transport that this crate does not recognise.
    #[error("unknown mail transport `{0}`")]
    UnknownTransport(String),

    /// A mailer names a transport that is recognised but not implemented
    /// (`ses`, `postmark`, `resend`, `sendmail`).
    #[error("mail transport `{0}` is not supported by rustasea-mail")]
    UnsupportedTransport(String),

    /// The `smtp` transport was selected but its `host` is missing or empty.
    #[error("smtp mailer `{0}` requires a non-empty host")]
    MissingSmtpHost(String),

    /// The `smtp` transport was selected but the crate was built without the
    /// `smtp` feature.
    #[error("mail transport `smtp` requires the `smtp` feature of rustasea-mail")]
    MissingSmtpFeature,

    /// A `failover` mailer lists no member mailers.
    #[error("failover mailer `{0}` lists no member mailers")]
    EmptyFailover(String),
}
