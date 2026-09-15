//! RustaSea timezone — IANA validation, a user-timezone resolution chain, and
//! UTC ↔ local formatting helpers.
//!
//! This crate is the RustaSea analogue of `glhd/laravel-timezone-mapper`: it
//! resolves a per-user timezone from a deterministic fallback chain and
//! provides the conversions the scheduler and views need to run on / display a
//! user's wall clock while every instant is stored in UTC.
//!
//! # Resolution chain
//!
//! [`resolve`] returns the first **valid** candidate in this order:
//!
//! 1. the user's stored preference (e.g. `users.timezone`),
//! 2. the session value,
//! 3. the request header (e.g. `X-Timezone`),
//! 4. the application default (`AppConfig.timezone`).
//!
//! Any `None`, empty, or invalid candidate is skipped — the chain falls through
//! rather than failing. When the application default is itself invalid the
//! result is [`DEFAULT_TIMEZONE`] (`"UTC"`); the resolver never panics.
//!
//! # Validation
//!
//! [`validate`] trims the name and accepts either an IANA identifier
//! (`Asia/Jakarta`) or one of a small set of short aliases documented on the
//! internal alias table (`PST`/`EST`/`CST`/`MST`/`GMT`). `UTC` resolves
//! natively. Everything else is rejected with [`TimezoneError::InvalidTimezone`].
//!
//! # Example
//!
//! ```
//! use rustasea_timezone::{resolve, validate, to_local, format_local};
//! use chrono::{DateTime, Utc};
//!
//! // User preference wins; an invalid candidate falls through.
//! assert_eq!(
//!     resolve(Some("Asia/Jakarta"), Some("America/New_York"), None, "UTC"),
//!     "Asia/Jakarta"
//! );
//! assert_eq!(resolve(Some("Not/AZone"), None, None, "UTC"), "UTC");
//!
//! // Format a UTC instant in the user's local wall clock.
//! let tz = validate("Asia/Jakarta").expect("valid zone");
//! let dt = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
//!     .expect("valid rfc3339")
//!     .with_timezone(&Utc);
//! assert_eq!(format_local(dt, tz, "%Y-%m-%d %H:%M"), "2026-01-01 07:00");
//! ```
//!
//! # Process-wide default
//!
//! Install a default once at boot and read it from anywhere:
//!
//! ```
//! use rustasea_timezone::{clear_default_timezone, default_timezone, set_default_timezone};
//!
//! clear_default_timezone();
//! assert_eq!(default_timezone(), "UTC");
//! set_default_timezone("Asia/Jakarta").expect("valid zone");
//! assert_eq!(default_timezone(), "Asia/Jakarta");
//! clear_default_timezone();
//! ```

pub mod error;
pub mod global;
pub mod mapper;

pub use error::{Result, TimezoneError};
pub use global::{clear_default_timezone, default_timezone, set_default_timezone};
pub use mapper::{
    format_local, now_local, resolve, resolve_tz, to_local, validate, TimezoneMapper,
    DEFAULT_TIMEZONE,
};

/// Re-export of `chrono-tz`'s [`Tz`](chrono_tz::Tz) so downstream crates can
/// name a parsed timezone without depending on `chrono-tz` directly.
pub use chrono_tz::Tz;

#[cfg(test)]
mod tests;
