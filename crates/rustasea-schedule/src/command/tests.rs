use super::*;
use chrono::{DateTime, Utc};

/// Parse a UTC instant from RFC 3339.
fn utc(rfc3339: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(rfc3339)
        .expect("valid rfc3339")
        .with_timezone(&Utc)
}

/// `daily_at("08:00")` in Asia/Jakarta fires at 01:00 UTC (UTC+7, no DST).
#[test]
fn daily_at_in_jakarta_maps_to_utc() {
    let cmd = ScheduleCommand::daily_at("emails:send", "08:00")
        .expect("valid schedule")
        .with_timezone("Asia/Jakarta")
        .expect("valid timezone");
    let next = cmd.next_run(utc("2026-01-01T00:00:00Z")).expect("a match");
    assert_eq!(next, utc("2026-01-01T01:00:00Z"));
}

/// The same `08:00` wall clock is a different UTC instant in winter (EST)
/// and summer (EDT) because of DST.
#[test]
fn daily_at_in_new_york_respects_dst() {
    let cmd = ScheduleCommand::daily_at("emails:send", "08:00")
        .expect("valid schedule")
        .with_timezone("America/New_York")
        .expect("valid timezone");
    // Winter: 08:00 EST (UTC-5) == 13:00 UTC.
    assert_eq!(
        cmd.next_run(utc("2026-01-15T00:00:00Z")).expect("a match"),
        utc("2026-01-15T13:00:00Z")
    );
    // Summer: 08:00 EDT (UTC-4) == 12:00 UTC.
    assert_eq!(
        cmd.next_run(utc("2026-07-15T00:00:00Z")).expect("a match"),
        utc("2026-07-15T12:00:00Z")
    );
}

/// A local time that falls in the spring-forward DST gap is skipped and the
/// scan continues to the next existing wall-clock occurrence.
#[test]
fn spring_forward_gap_is_skipped() {
    // 02:30 does not exist on 2026-03-08 in America/New_York.
    let cmd = ScheduleCommand::cron("emails:send", "30 2 * * *")
        .expect("valid schedule")
        .with_timezone("America/New_York")
        .expect("valid timezone");
    let next = cmd.next_run(utc("2026-03-08T00:00:00Z")).expect("a match");
    // The gap is skipped; the next 02:30 is 2026-03-09 EDT (UTC-4).
    assert_eq!(next, utc("2026-03-09T06:30:00Z"));
}

/// An ambiguous fall-back local time resolves to its earliest occurrence when
/// the scan starts before the overlap.
#[test]
fn fall_back_overlap_uses_earliest() {
    // 01:30 occurs twice on 2026-11-01 in America/New_York.
    let cmd = ScheduleCommand::cron("emails:send", "30 1 * * *")
        .expect("valid schedule")
        .with_timezone("America/New_York")
        .expect("valid timezone");
    let next = cmd.next_run(utc("2026-11-01T00:00:00Z")).expect("a match");
    // Earliest = 01:30 EDT (UTC-4) == 05:30 UTC.
    assert_eq!(next, utc("2026-11-01T05:30:00Z"));
}

/// Regression (TASK-059): with `from` inside the first (EDT) occurrence of the
/// fall-back overlap, the next `* * * * *` tick must still be strictly after
/// `from`, not the ambiguous minute mapped backwards.
#[test]
fn fall_back_from_first_occurrence_is_strictly_future() {
    // 2026-11-01 05:30 UTC == 01:30 EDT, the first of the two 01:30s.
    let cmd = ScheduleCommand::every_minute("emails:send")
        .expect("valid schedule")
        .with_timezone("America/New_York")
        .expect("valid timezone");
    let from = utc("2026-11-01T05:30:00Z");
    let next = cmd.next_run(from).expect("a match");
    assert!(
        next > from,
        "next tick {next} must be strictly after from {from}"
    );
    assert_eq!(next, utc("2026-11-01T05:31:00Z"));
}

/// Regression (TASK-059): with `from` inside the second (EST) occurrence of the
/// fall-back overlap, the next `* * * * *` tick must be strictly after `from`.
/// Before the fix the tz path mapped 01:31 local back with `.earliest()`,
/// yielding 05:31 UTC — an hour in the past relative to a 06:30 UTC start.
#[test]
fn fall_back_from_second_occurrence_is_strictly_future() {
    // 2026-11-01 06:00 UTC == 01:00 EST, the second of the two 01:00s.
    let cmd = ScheduleCommand::every_minute("emails:send")
        .expect("valid schedule")
        .with_timezone("America/New_York")
        .expect("valid timezone");
    let from = utc("2026-11-01T06:00:00Z");
    let next = cmd.next_run(from).expect("a match");
    assert!(
        next > from,
        "next tick {next} must be strictly after from {from}"
    );
    assert_eq!(next, utc("2026-11-01T06:01:00Z"));
}

/// Regression (TASK-059): an hourly expression started inside the first (EDT)
/// occurrence of the fall-back overlap advances strictly into the future.
#[test]
fn fall_back_hourly_from_first_occurrence_advances() {
    let cmd = ScheduleCommand::cron("emails:send", "0 * * * *")
        .expect("valid schedule")
        .with_timezone("America/New_York")
        .expect("valid timezone");
    let from = utc("2026-11-01T05:30:00Z");
    let next = cmd.next_run(from).expect("a match");
    assert!(
        next > from,
        "next hourly tick {next} must be strictly after from {from}"
    );
    // The scan walks the wall clock forward, so the next whole local hour it
    // reaches is 02:00 EST == 07:00 UTC.
    assert_eq!(next, utc("2026-11-01T07:00:00Z"));
}

/// Regression (TASK-059): an hourly expression started inside the second (EST)
/// occurrence of the fall-back overlap advances strictly into the future
/// instead of returning the first occurrence in the past.
#[test]
fn fall_back_hourly_from_second_occurrence_advances() {
    let cmd = ScheduleCommand::cron("emails:send", "0 * * * *")
        .expect("valid schedule")
        .with_timezone("America/New_York")
        .expect("valid timezone");
    let from = utc("2026-11-01T06:00:00Z");
    let next = cmd.next_run(from).expect("a match");
    assert!(
        next > from,
        "next hourly tick {next} must be strictly after from {from}"
    );
    // The next whole local hour is 02:00 EST == 07:00 UTC.
    assert_eq!(next, utc("2026-11-01T07:00:00Z"));
}

/// A cron expression in a timezone evaluates the same fields against the
/// zone's wall clock, not UTC.
#[test]
fn cron_in_timezone_differs_from_utc() {
    let tz_cmd = ScheduleCommand::cron("emails:send", "0 12 * * *")
        .expect("valid schedule")
        .with_timezone("Asia/Jakarta")
        .expect("valid timezone");
    let utc_cmd = ScheduleCommand::cron("emails:send", "0 12 * * *").expect("valid schedule");
    let from = utc("2026-01-01T00:00:00Z");
    assert_eq!(
        tz_cmd.next_run(from).expect("a match"),
        utc("2026-01-01T05:00:00Z")
    );
    assert_eq!(
        utc_cmd.next_run(from).expect("a match"),
        utc("2026-01-01T12:00:00Z")
    );
}

/// An unknown timezone name is rejected with a typed error, not a panic.
#[test]
fn invalid_timezone_is_rejected() {
    let err = ScheduleCommand::daily_at("emails:send", "08:00")
        .expect("valid schedule")
        .with_timezone("Not/AZone")
        .expect_err("invalid timezone is rejected");
    assert!(matches!(err, ScheduleError::Invalid(_)));
}

/// A short alias (`PST`) resolves and behaves like its canonical zone.
#[test]
fn alias_timezone_resolves() {
    let cmd = ScheduleCommand::daily_at("emails:send", "08:00")
        .expect("valid schedule")
        .with_timezone("PST")
        .expect("valid timezone");
    assert_eq!(cmd.timezone.as_deref(), Some("America/Los_Angeles"));
}

/// With no timezone the command behaves exactly as before (UTC-only).
#[test]
fn default_timezone_is_utc_unchanged() {
    let cmd = ScheduleCommand::daily_at("emails:send", "08:00").expect("valid schedule");
    assert_eq!(cmd.timezone, None);
    assert_eq!(
        cmd.next_run(utc("2026-01-01T00:00:00Z")).expect("a match"),
        utc("2026-01-01T08:00:00Z")
    );
}

/// The builder threads `.timezone(...)` through to the built command.
#[test]
fn builder_timezone_threads_through() {
    let cmd = crate::builder::Schedule::command("emails:send")
        .daily()
        .at("08:00")
        .timezone("Asia/Jakarta")
        .expect("valid timezone")
        .build()
        .expect("valid build");
    assert_eq!(cmd.timezone.as_deref(), Some("Asia/Jakarta"));
    assert_eq!(
        cmd.next_run(utc("2026-01-01T00:00:00Z")).expect("a match"),
        utc("2026-01-01T01:00:00Z")
    );
}

/// An invalid builder timezone surfaces a typed error at the chain step.
#[test]
fn builder_rejects_invalid_timezone() {
    let err = crate::builder::Schedule::command("emails:send")
        .daily()
        .timezone("Not/AZone")
        .expect_err("invalid timezone is rejected");
    assert!(matches!(err, ScheduleError::Invalid(_)));
}
