//! Process-wide default timezone registry.
//!
//! Mirrors the mailer/i18n registry precedent: a
//! `OnceLock<RwLock<Option<String>>>` slot with a validating `set_*` installer,
//! an accessor, and a `clear_*` reset. Application boot installs one default
//! (typically from `AppConfig.timezone`); request handlers and helpers then read
//! it without threading the value through every signature.
//!
//! Every access is poison-tolerant and never panics: a poisoned lock is
//! recovered via `into_inner`, and [`default_timezone`] falls back to
//! [`DEFAULT_TIMEZONE`] when nothing is installed or the installed name is
//! somehow invalid.

use std::sync::{OnceLock, RwLock};

use crate::error::Result;
use crate::mapper::{validate, DEFAULT_TIMEZONE};

/// Registry slot holding the process-wide default timezone name.
static DEFAULT_TZ: OnceLock<RwLock<Option<String>>> = OnceLock::new();

/// Access the lazily-initialized default-timezone slot.
fn slot() -> &'static RwLock<Option<String>> {
    DEFAULT_TZ.get_or_init(|| RwLock::new(None))
}

/// Install the process-wide default timezone.
///
/// The name is validated before it is stored, so an invalid name is rejected
/// with [`crate::TimezoneError::InvalidTimezone`] and the previously installed
/// value (if any) is left untouched.
///
/// # Errors
///
/// [`crate::TimezoneError::InvalidTimezone`] when `name` does not resolve.
pub fn set_default_timezone(name: &str) -> Result<()> {
    let tz = validate(name)?;
    *slot().write().unwrap_or_else(|p| p.into_inner()) = Some(tz.name().to_string());
    Ok(())
}

/// The process-wide default timezone name.
///
/// Falls back to [`DEFAULT_TIMEZONE`] (`UTC`) when nothing is installed or the
/// stored value no longer validates.
pub fn default_timezone() -> String {
    let stored = slot().read().unwrap_or_else(|p| p.into_inner()).clone();
    match stored {
        Some(name) if validate(&name).is_ok() => name,
        _ => DEFAULT_TIMEZONE.to_string(),
    }
}

/// Remove the process-wide default timezone, if any.
pub fn clear_default_timezone() {
    *slot().write().unwrap_or_else(|p| p.into_inner()) = None;
}
