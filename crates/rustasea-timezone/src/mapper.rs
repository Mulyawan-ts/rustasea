//! Timezone validation, resolution, and local-time formatting.
//!
//! Mirrors the intent of `glhd/laravel-timezone-mapper`: a per-user timezone is
//! resolved from a deterministic fallback chain — **user preference → session →
//! request header → application default** — and any wall-clock conversion goes
//! through that single resolved zone. Every step is fail-open to the next valid
//! candidate (never a panic), and the final application default falls back to
//! [`DEFAULT_TIMEZONE`] when it is itself invalid.

use std::str::FromStr;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::error::{Result, TimezoneError};

/// Canonical fallback zone used when nothing else resolves.
pub const DEFAULT_TIMEZONE: &str = "UTC";

/// Small short-name alias table for `glhd/laravel-timezone-mapper` parity.
///
/// The upstream package accepts the common North-American abbreviations (and
/// `GMT`) in addition to full IANA identifiers. Aliases are matched
/// case-insensitively; the canonical IANA name on the right is what actually
/// resolves. `"UTC"` is handled natively by `chrono-tz`, so it is not listed.
///
/// | Alias | Canonical IANA zone |
/// |-------|---------------------|
/// | `PST` | `America/Los_Angeles` |
/// | `EST` | `America/New_York` |
/// | `CST` | `America/Chicago` |
/// | `MST` | `America/Denver` |
/// | `GMT` | `Etc/GMT` |
const ALIASES: &[(&str, &str)] = &[
    ("PST", "America/Los_Angeles"),
    ("EST", "America/New_York"),
    ("CST", "America/Chicago"),
    ("MST", "America/Denver"),
    ("GMT", "Etc/GMT"),
];

/// Validate a timezone name and return its parsed [`Tz`].
///
/// The name is trimmed of surrounding whitespace, then matched against the
/// short-alias table (case-insensitively) and, failing that, parsed as an IANA
/// identifier via [`Tz::from_str`]. An empty name, an unknown alias, or a name
/// that is not a valid IANA zone yields [`TimezoneError::InvalidTimezone`].
///
/// # Errors
///
/// [`TimezoneError::InvalidTimezone`] for any name that does not resolve.
pub fn validate(name: &str) -> Result<Tz> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(TimezoneError::InvalidTimezone {
            name: name.to_string(),
            reason: "timezone name is empty".to_string(),
        });
    }
    let canonical = ALIASES
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(trimmed))
        .map(|(_, canonical)| *canonical)
        .unwrap_or(trimmed);
    Tz::from_str(canonical).map_err(|_| TimezoneError::InvalidTimezone {
        name: name.to_string(),
        reason: format!("`{trimmed}` is not a known IANA timezone"),
    })
}

/// Resolve the effective timezone name from a fallback chain.
///
/// Returns the **first valid** candidate in the order `user → session → header
/// → app_default`, skipping any that is `None`, empty, or invalid (the chain
/// falls through rather than failing). When `app_default` is itself invalid the
/// result is [`DEFAULT_TIMEZONE`] — this function never panics and never returns
/// an unparsable name.
pub fn resolve(
    user: Option<&str>,
    session: Option<&str>,
    header: Option<&str>,
    app_default: &str,
) -> String {
    for name in [user, session, header].into_iter().flatten() {
        if validate(name).is_ok() {
            return name.trim().to_string();
        }
    }
    if validate(app_default).is_ok() {
        app_default.trim().to_string()
    } else {
        DEFAULT_TIMEZONE.to_string()
    }
}

/// Convenience wrapper over [`resolve`] that returns the parsed [`Tz`].
///
/// The returned zone is always valid: an unresolvable chain yields
/// [`DEFAULT_TIMEZONE`] (`UTC`).
pub fn resolve_tz(
    user: Option<&str>,
    session: Option<&str>,
    header: Option<&str>,
    app_default: &str,
) -> Tz {
    // `resolve` only ever returns a name that `validate` accepted (or `UTC`,
    // which is always valid), so the fallback below is unreachable in practice.
    validate(&resolve(user, session, header, app_default)).unwrap_or(chrono_tz::UTC)
}

/// Convert a UTC instant into `tz` local time.
pub fn to_local(dt: DateTime<Utc>, tz: Tz) -> DateTime<Tz> {
    dt.with_timezone(&tz)
}

/// Current instant expressed in `tz` local time.
pub fn now_local(tz: Tz) -> DateTime<Tz> {
    Utc::now().with_timezone(&tz)
}

/// Format a UTC instant in `tz` using a `strftime` pattern.
///
/// An invalid pattern (a `chrono` format error) degrades to the RFC 3339
/// representation of the local instant rather than panicking.
pub fn format_local(dt: DateTime<Utc>, tz: Tz, pattern: &str) -> String {
    let local = to_local(dt, tz);
    let items: Vec<chrono::format::Item<'_>> =
        chrono::format::StrftimeItems::new(pattern).collect();
    if items
        .iter()
        .any(|item| matches!(item, chrono::format::Item::Error))
    {
        return local.to_rfc3339();
    }
    local.format_with_items(items.iter()).to_string()
}

/// Stateful resolver binding an application-default zone.
///
/// A thin wrapper over the free functions so application code can hold one
/// configured mapper (built from `AppConfig.timezone`) and call
/// [`TimezoneMapper::resolve`] per request without threading the default
/// through every signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimezoneMapper {
    /// Application-default timezone name (validated on construction).
    app_default: String,
}

impl TimezoneMapper {
    /// Build a mapper with `app_default` as the last-resort zone.
    ///
    /// The default is validated immediately; an invalid name is normalized to
    /// [`DEFAULT_TIMEZONE`] so the mapper is always usable.
    pub fn new(app_default: impl Into<String>) -> Self {
        let app_default = app_default.into();
        let app_default = if validate(&app_default).is_ok() {
            app_default.trim().to_string()
        } else {
            DEFAULT_TIMEZONE.to_string()
        };
        Self { app_default }
    }

    /// The configured application-default timezone name.
    pub fn app_default(&self) -> &str {
        &self.app_default
    }

    /// Resolve the effective timezone name for a request.
    pub fn resolve(
        &self,
        user: Option<&str>,
        session: Option<&str>,
        header: Option<&str>,
    ) -> String {
        resolve(user, session, header, &self.app_default)
    }

    /// Resolve the effective timezone as a parsed [`Tz`].
    pub fn resolve_tz(
        &self,
        user: Option<&str>,
        session: Option<&str>,
        header: Option<&str>,
    ) -> Tz {
        resolve_tz(user, session, header, &self.app_default)
    }
}
