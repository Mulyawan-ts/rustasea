//! Unit tests for `log:show` argument parsing and the follow/tail core.
//!
//! These cover the pieces that are awkward to drive end-to-end: the flag
//! parser, the incremental tail cursor, the streaming [`StreamSink`] (which
//! must write straight through without touching the in-process buffer), and the
//! rotation-aware [`Follower`] (which switches to a newer dated sibling).

use super::*;
use std::fs::OpenOptions;
use std::path::Path;

use rustasea_logging::ChannelConfig;

/// Build a `daily` config whose channel points at `dir/rustasea.log`.
///
/// The bare path is intentionally not created by callers that exercise
/// rotation, so [`resolve_log_file`] falls back to the newest dated sibling.
fn daily_config(dir: &Path) -> LoggingConfig {
    let path = dir.join("rustasea.log");
    LoggingConfig::new("daily").with_channel(
        "daily",
        ChannelConfig::new("daily").with_path(path.to_string_lossy().into_owned()),
    )
}

/// Both flag spellings parse, and the toggles are recognised.
#[test]
fn parses_flags_both_spellings() {
    let options = parse_args(&[
        "--level".into(),
        "error".into(),
        "--channel=daily".into(),
        "--since".into(),
        "2026-09-15T00:00:00Z".into(),
        "--grep=boom".into(),
        "--limit".into(),
        "5".into(),
        "--follow".into(),
        "--json".into(),
    ])
    .expect("valid args");
    assert_eq!(options.level, Some(LogLevel::Error));
    assert_eq!(options.channel.as_deref(), Some("daily"));
    assert_eq!(options.grep.as_deref(), Some("boom"));
    assert_eq!(options.limit, Some(5));
    assert!(options.follow);
    assert!(options.json);
    assert!(options.since.is_some());
}

/// Bad values and unknown flags are typed argument errors.
#[test]
fn rejects_invalid_arguments() {
    for bad in [
        vec!["--level".to_string(), "bogus".to_string()],
        vec!["--since=bogus".to_string()],
        vec!["--limit=x".to_string()],
        vec!["--bogus".to_string()],
        vec!["--level".to_string()],
    ] {
        let error = parse_args(&bad).expect_err("must fail");
        assert!(
            matches!(error, CliError::InvalidArguments { .. }),
            "{error:?}"
        );
    }
}

/// The incremental tail core emits only complete appended lines.
#[test]
fn follower_emits_only_complete_appended_lines() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("rustasea.log");
    std::fs::write(&path, "2026-09-15T00:00:00Z INFO app: one\n").unwrap();

    let mut follower = Follower::new(path.clone(), 0, None);
    let mut io = Io::default();
    let count = follower
        .tail_current(&LogQuery::default(), false, &mut io)
        .unwrap();
    assert_eq!(count, 1);
    assert!(io.stdout.contains("one"), "stdout: {}", io.stdout);

    // Append one complete and one partial line; only the complete one emits.
    {
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        write!(
            file,
            "2026-09-15T00:00:01Z WARN app: two\n2026-09-15T00:00:02Z ERR"
        )
        .unwrap();
    }
    let mut io2 = Io::default();
    let count = follower
        .tail_current(&LogQuery::default(), false, &mut io2)
        .unwrap();
    assert_eq!(count, 1);
    assert!(io2.stdout.contains("two"), "stdout: {}", io2.stdout);
}

/// The `--json` tail core emits one parseable object per line.
#[test]
fn follower_json_is_one_object_per_line() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("rustasea.log");
    std::fs::write(&path, "2026-09-15T00:00:00Z INFO app: hi\n").unwrap();

    let mut follower = Follower::new(path, 0, None);
    let mut io = Io::default();
    follower
        .tail_current(&LogQuery::default(), true, &mut io)
        .unwrap();

    let line = io.stdout.trim();
    let parsed: serde_json::Value = serde_json::from_str(line).expect("valid JSON");
    assert_eq!(parsed["level"], "INFO");
    assert_eq!(parsed["message"], "hi");
}

/// Streaming writes straight through while the in-process buffer stays empty.
///
/// This is the TASK-081 guarantee: with `--follow`, lines reach the terminal
/// via [`StreamSink`] and are never accumulated in the `Io` buffer that
/// `Artisan::call` would hold until the loop ends.
#[test]
fn streaming_writes_through_and_leaves_buffer_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("rustasea.log");
    std::fs::write(&path, "2026-09-15T00:00:00Z INFO app: streamed\n").unwrap();
    let config = daily_config(dir.path());

    // Stand-in for the in-process buffer `Artisan::call` returns; it must not
    // receive any followed line.
    let io = Io::default();
    let mut wire: Vec<u8> = Vec::new();
    let mut follower = Follower::new(path, 0, Some("daily".to_string()));

    {
        let mut sink = StreamSink::new(&mut wire);
        let emitted = follower
            .tick(&config, &LogQuery::default(), false, &mut sink)
            .unwrap();
        assert_eq!(emitted, 1);
    }

    assert!(
        String::from_utf8_lossy(&wire).contains("streamed"),
        "streamed output missing"
    );
    assert!(io.stdout.is_empty(), "io.stdout: {}", io.stdout);
}

/// A broken pipe is treated as a clean end, not a failure.
#[test]
fn broken_pipe_is_clean_exit() {
    let error = CliError::Io(std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "pipe closed",
    ));
    assert!(is_broken_pipe(&error));
    assert!(stop_on_broken_pipe(error).is_ok());

    let other = CliError::Io(std::io::Error::other("boom"));
    assert!(!is_broken_pipe(&other));
    assert!(stop_on_broken_pipe(other).is_err());
}

/// Follow switches to a newer dated sibling and does not lose the old tail.
#[test]
fn follower_switches_to_rotated_sibling() {
    let dir = tempfile::tempdir().expect("tempdir");
    let old = dir.path().join("rustasea-.2026-09-15.log");
    std::fs::write(&old, "2026-09-15T23:59:59Z INFO app: old-one\n").unwrap();
    let config = daily_config(dir.path());

    // With no bare file, the resolver selects the newest (only) sibling.
    let resolved = resolve_log_file(&config, Some("daily")).expect("resolve");
    assert_eq!(resolved, old);

    let mut wire: Vec<u8> = Vec::new();
    let mut follower = Follower::new(resolved, 0, Some("daily".to_string()));

    // First tick: emit the old file's backlog.
    {
        let mut sink = StreamSink::new(&mut wire);
        let emitted = follower
            .tick(&config, &LogQuery::default(), false, &mut sink)
            .unwrap();
        assert_eq!(emitted, 1);
    }
    assert!(String::from_utf8_lossy(&wire).contains("old-one"));

    // Append a trailing line to the old file, then rotate to a newer sibling.
    {
        let mut file = OpenOptions::new().append(true).open(&old).unwrap();
        writeln!(file, "2026-09-15T23:59:59Z INFO app: old-two").unwrap();
    }
    let new = dir.path().join("rustasea-.2026-09-16.log");
    std::fs::write(&new, "2026-09-16T00:00:01Z INFO app: new-one\n").unwrap();

    // Next tick: drain the old file's trailing line, switch, emit the new line.
    {
        let mut sink = StreamSink::new(&mut wire);
        let emitted = follower
            .tick(&config, &LogQuery::default(), false, &mut sink)
            .unwrap();
        assert_eq!(emitted, 2);
    }

    let out = String::from_utf8_lossy(&wire);
    assert!(out.contains("old-two"), "old tail lost: {out}");
    assert!(out.contains("new-one"), "new sibling missed: {out}");
    assert_eq!(follower.path, new);
}
