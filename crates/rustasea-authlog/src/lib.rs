//! RustaSea authentication log — sign-in history with an optional new-device
//! notification (rappasoft/laravel-authentication-log parity).
//!
//! The log records one row per authentication occurrence — a successful login,
//! a failed attempt, a rate-limiter lockout, and a logout — with the client IP
//! and `User-Agent` so an account owner can audit where their account was used.
//! After a successful login from an IP the account has never used, the
//! configured [`NewDeviceNotifier`] is invoked; the shipped
//! [`QueuedMailNotifier`] queues a [`NewDeviceNotification`] through
//! `rustasea-mail` (never inline, so mail cannot block a login).
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use rustasea_authlog::{AuthLogEvent, AuthenticationLogLogger};
//!
//! // Install once at boot:
//! rustasea_authlog::install(Arc::new(AuthenticationLogLogger::new(pool.clone())));
//!
//! // Record an event (best-effort; no-op when no logger is installed):
//! rustasea_authlog::record_event(&AuthLogEvent::login_succeeded(
//!     user_id, Some(email), Some("session"), Some(ip), Some(ua),
//! ))
//! .await?;
//!
//! // Query the history:
//! let logger = AuthenticationLogLogger::new(pool.clone());
//! let rows = logger.for_user("user-1").await?;
//! ```
//!
//! ## Best-effort recording
//!
//! The HTTP layer treats recording as best-effort: a storage or mail failure is
//! logged, never surfaced as a login failure. Authentication that already
//! succeeded must not be undone by an audit-write hiccup.

mod error;
mod event;
mod global;
mod migration;
mod model;
mod notification;
mod notifier;
mod recorder;

#[cfg(test)]
mod tests;

pub use error::{AuthLogError, Result};
pub use event::{AuthLogEvent, AuthLogEventKind};
pub use global::{clear, install, logger, record_event};
pub use migration::{register, CreateAuthenticationLogTable};
pub use model::AuthenticationLog;
pub use notification::NewDeviceNotification;
pub use notifier::{NewDeviceNotifier, QueuedMailNotifier};
pub use recorder::AuthenticationLogLogger;

/// Default `authentication_log` table name.
pub const AUTHENTICATION_LOG_TABLE: &str = "authentication_log";
