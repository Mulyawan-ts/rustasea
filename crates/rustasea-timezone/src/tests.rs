//! Unit tests for the timezone mapper, resolver, formatting, and global slot.

use chrono::{DateTime, Datelike, Offset, Timelike, Utc};
use chrono_tz::{Tz, UTC};

use super::*;

/// Parse a UTC instant from RFC 3339 for the assertions below.
fn utc(rfc3339: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(rfc3339)
        .expect("valid rfc3339")
        .with_timezone(&Utc)
}

/// `validate` accepts IANA names, aliases, and trims whitespace.
#[test]
fn validate_accepts_iana_aliases_and_trims() {
    assert_eq!(validate("Asia/Jakarta").expect("valid"), Tz::Asia__Jakarta);
    assert_eq!(validate("UTC").expect("valid"), UTC);
    // Surrounding whitespace is trimmed before parsing.
    assert_eq!(
        validate("  America/New_York  ").expect("valid"),
        Tz::America__New_York
    );
    // Short aliases resolve to their canonical zone, case-insensitively.
    assert_eq!(validate("PST").expect("valid"), Tz::America__Los_Angeles);
    assert_eq!(validate("pst").expect("valid"), Tz::America__Los_Angeles);
    assert_eq!(validate("EST").expect("valid"), Tz::America__New_York);
    assert_eq!(validate("CST").expect("valid"), Tz::America__Chicago);
    assert_eq!(validate("MST").expect("valid"), Tz::America__Denver);
    assert_eq!(validate("GMT").expect("valid"), Tz::Etc__GMT);
}

/// `validate` rejects empty and unknown names with a typed error.
#[test]
fn validate_rejects_empty_and_unknown() {
    let empty = validate("   ").expect_err("empty is rejected");
    assert_eq!(empty.code(), "TimezoneError::InvalidTimezone");
    let unknown = validate("Not/AZone").expect_err("unknown is rejected");
    assert_eq!(unknown.code(), "TimezoneError::InvalidTimezone");
    assert!(matches!(
        unknown,
        TimezoneError::InvalidTimezone { ref name, .. } if name == "Not/AZone"
    ));
}

/// The resolver returns the first valid candidate in chain order.
#[test]
fn resolve_prefers_user_then_falls_through() {
    // User wins when valid.
    assert_eq!(
        resolve(Some("Asia/Jakarta"), Some("America/New_York"), None, "UTC"),
        "Asia/Jakarta"
    );
    // An invalid user candidate falls through to the session value.
    assert_eq!(
        resolve(Some("Not/AZone"), Some("America/New_York"), None, "UTC"),
        "America/New_York"
    );
    // An empty user candidate is skipped too.
    assert_eq!(
        resolve(Some(""), Some("America/New_York"), None, "UTC"),
        "America/New_York"
    );
    // User + session invalid falls through to the header.
    assert_eq!(
        resolve(
            Some("Not/AZone"),
            Some("Also/Bad"),
            Some("Europe/Paris"),
            "UTC"
        ),
        "Europe/Paris"
    );
    // Only the default remains.
    assert_eq!(resolve(None, None, None, "Europe/Paris"), "Europe/Paris");
    // An invalid default falls back to UTC.
    assert_eq!(resolve(None, None, None, "Not/AZone"), "UTC");
}

/// `resolve_tz` yields a parsed zone that always matches the resolved name.
#[test]
fn resolve_tz_returns_parsed_zone() {
    assert_eq!(
        resolve_tz(Some("Asia/Jakarta"), None, None, "UTC"),
        Tz::Asia__Jakarta
    );
    // Unresolvable chain still yields a usable zone (UTC).
    assert_eq!(resolve_tz(Some("bad"), None, None, "also/bad"), UTC);
}

/// `to_local` shifts the UTC instant into the target zone's offset.
#[test]
fn to_local_applies_the_offset() {
    let tz = validate("Asia/Jakarta").expect("valid");
    let local = to_local(utc("2026-01-01T00:00:00Z"), tz);
    // Jakarta is UTC+7 year-round.
    assert_eq!(local.hour(), 7);
    assert_eq!(local.day(), 1);
    assert_eq!(local.offset().fix().local_minus_utc(), 7 * 3600);
}

/// `format_local` renders a wall-clock string in the target zone.
#[test]
fn format_local_renders_wall_clock() {
    let tz = validate("Asia/Jakarta").expect("valid");
    assert_eq!(
        format_local(utc("2026-01-01T00:00:00Z"), tz, "%Y-%m-%d %H:%M"),
        "2026-01-01 07:00"
    );
    // An invalid pattern degrades to RFC 3339 instead of panicking.
    let fallback = format_local(utc("2026-01-01T00:00:00Z"), tz, "%Q");
    assert!(fallback.contains("2026-01-01T07:00:00"));
}

/// A daily `08:00` New York wall-clock is a different UTC instant in winter vs
/// summer because of DST (EST = UTC-5, EDT = UTC-4).
#[test]
fn format_local_respects_dst_offsets() {
    let ny = validate("America/New_York").expect("valid");
    // Winter (EST, UTC-5): 08:00 local == 13:00 UTC.
    let winter = to_local(utc("2026-01-15T13:00:00Z"), ny);
    assert_eq!((winter.hour(), winter.minute()), (8, 0));
    assert_eq!(winter.offset().fix().local_minus_utc(), -5 * 3600);
    // Summer (EDT, UTC-4): 08:00 local == 12:00 UTC.
    let summer = to_local(utc("2026-07-15T12:00:00Z"), ny);
    assert_eq!((summer.hour(), summer.minute()), (8, 0));
    assert_eq!(summer.offset().fix().local_minus_utc(), -4 * 3600);
}

/// `now_local` returns the current instant in the requested zone.
#[test]
fn now_local_is_current_instant() {
    let tz = validate("Asia/Jakarta").expect("valid");
    let local = now_local(tz);
    assert_eq!(local.timezone(), tz);
}

/// The `TimezoneMapper` wrapper normalizes an invalid default and delegates.
#[test]
fn mapper_wraps_the_free_functions() {
    let mapper = TimezoneMapper::new("Asia/Jakarta");
    assert_eq!(mapper.app_default(), "Asia/Jakarta");
    assert_eq!(
        mapper.resolve(Some("Europe/Paris"), None, None),
        "Europe/Paris"
    );
    assert_eq!(mapper.resolve(None, None, None), "Asia/Jakarta");
    assert_eq!(mapper.resolve_tz(None, None, None), Tz::Asia__Jakarta);

    // An invalid configured default normalizes to UTC.
    let fallback = TimezoneMapper::new("Not/AZone");
    assert_eq!(fallback.app_default(), "UTC");
    assert_eq!(fallback.resolve(None, None, None), "UTC");
}

/// The global slot validates on install and falls back to UTC when cleared.
#[test]
fn global_default_slot_validates_and_falls_back() {
    // Serialize with any other test touching the process-wide slot.
    let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    clear_default_timezone();
    assert_eq!(default_timezone(), "UTC");

    set_default_timezone("Asia/Jakarta").expect("valid zone");
    assert_eq!(default_timezone(), "Asia/Jakarta");

    // An invalid name is rejected and leaves the installed value untouched.
    assert!(set_default_timezone("Not/AZone").is_err());
    assert_eq!(default_timezone(), "Asia/Jakarta");

    clear_default_timezone();
    assert_eq!(default_timezone(), "UTC");
}

/// Serializes tests that mutate the process-wide default slot.
static GLOBAL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
