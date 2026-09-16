//! Reader unit tests — parsing, filtering, resolution, and incremental tailing.
//!
//! Split from [`super`] so `reader/mod.rs` stays under the 500-line cap. The
//! tests exercise real files in a `tempfile` directory; nothing touches the
//! process working directory.

use super::*;

/// Write `contents` to a unique temp file and return its directory + path.
fn temp_file(name: &str, contents: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(name);
    std::fs::write(&path, contents).expect("write");
    (dir, path)
}

/// Level, since, grep and limit filters each drop the right entries.
#[test]
fn query_filters_by_level_since_grep_and_limit() {
    let (dir, path) = temp_file(
        "rustasea.log",
        "2026-09-15T00:00:00Z INFO app: hello world\n\
         2026-09-15T01:00:00Z ERROR app: boom\n\
         2026-09-15T02:00:00Z WARN app: careful now\n\
         2026-09-15T03:00:00Z ERROR app: boom again\n",
    );

    let all = read_entries(&path, &LogQuery::default()).expect("read");
    assert_eq!(all.entries.len(), 4);
    assert_eq!(all.skipped, 0);

    let errors = LogQuery {
        min_level: Some(LogLevel::Error),
        ..Default::default()
    };
    let result = read_entries(&path, &errors).expect("read");
    assert_eq!(result.entries.len(), 2);
    assert!(result.entries.iter().all(|e| e.level == LogLevel::Error));

    let since = LogQuery {
        since: Some(
            DateTime::parse_from_rfc3339("2026-09-15T01:30:00Z")
                .unwrap()
                .with_timezone(&Utc),
        ),
        ..Default::default()
    };
    assert_eq!(read_entries(&path, &since).unwrap().entries.len(), 2);

    let grep = LogQuery {
        grep: Some("BOOM".to_string()),
        ..Default::default()
    };
    assert_eq!(read_entries(&path, &grep).unwrap().entries.len(), 2);

    let limited = LogQuery {
        limit: Some(1),
        ..Default::default()
    };
    let result = read_entries(&path, &limited).unwrap();
    assert_eq!(result.entries.len(), 1);
    assert_eq!(result.entries[0].message, "boom again");

    drop(dir);
}

/// Malformed lines are counted, not fatal.
#[test]
fn read_entries_counts_skipped() {
    let (_dir, path) = temp_file(
        "rustasea.log",
        "2026-09-15T00:00:00Z INFO app: ok\nnonsense line\n",
    );
    let result = read_entries(&path, &LogQuery::default()).unwrap();
    assert_eq!(result.entries.len(), 1);
    assert_eq!(result.skipped, 1);
}

/// A missing file is a typed error.
#[test]
fn read_entries_missing_file_errors() {
    let error = read_entries(Path::new("/nonexistent/rustasea.log"), &LogQuery::default())
        .expect_err("must fail");
    assert!(matches!(error, LoggingError::Io(_)), "{error:?}");
}

/// Exact path resolution wins when the file exists.
#[test]
fn resolve_prefers_exact_file() {
    let (dir, path) = temp_file("rustasea.log", "2026-01-01T00:00:00Z INFO app: hi\n");
    let config = LoggingConfig::default().with_channel(
        "single",
        crate::config::ChannelConfig::new("single").with_path(path.to_str().unwrap()),
    );
    let resolved = resolve_log_file(&config, Some("single")).unwrap();
    assert_eq!(resolved, path);
    drop(dir);
}

/// When the exact file is absent, the newest rotated sibling is returned.
#[test]
fn resolve_falls_back_to_latest_rotated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("rustasea.log");
    std::fs::write(dir.path().join("rustasea-.2026-09-14.log"), "old\n").unwrap();
    std::fs::write(dir.path().join("rustasea-.2026-09-15.log"), "new\n").unwrap();
    let config = LoggingConfig::default().with_channel(
        "daily",
        crate::config::ChannelConfig::new("daily").with_path(base.to_str().unwrap()),
    );
    let resolved = resolve_log_file(&config, Some("daily")).unwrap();
    assert_eq!(resolved.file_name().unwrap(), "rustasea-.2026-09-15.log");
}

/// A missing file with no rotated sibling is a typed error.
#[test]
fn resolve_missing_file_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("rustasea.log");
    let config = LoggingConfig::default().with_channel(
        "daily",
        crate::config::ChannelConfig::new("daily").with_path(base.to_str().unwrap()),
    );
    let error = resolve_log_file(&config, Some("daily")).expect_err("must fail");
    assert!(
        matches!(error, LoggingError::LogFileMissing { .. }),
        "{error:?}"
    );
}

/// An unknown channel name is a typed error.
#[test]
fn resolve_unknown_channel_errors() {
    let config = LoggingConfig::default();
    let error = resolve_log_file(&config, Some("nope")).expect_err("must fail");
    assert!(
        matches!(error, LoggingError::UnknownChannel(_)),
        "{error:?}"
    );
}

/// The incremental reader emits only complete appended lines and carries a
/// partial trailing line across calls.
#[test]
fn read_new_entries_tails_a_growing_file() {
    let (_dir, path) = temp_file("rustasea.log", "2026-09-15T00:00:00Z INFO app: one\n");
    let mut state = TailState::new(0);

    let first = read_new_entries(&mut state, &path, &LogQuery::default()).unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].message, "one");

    // Append a complete line plus a partial one.
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        write!(
            file,
            "2026-09-15T00:00:01Z WARN app: two\n2026-09-15T00:00:02Z ERR"
        )
        .unwrap();
    }
    let second = read_new_entries(&mut state, &path, &LogQuery::default()).unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].message, "two");

    // Finish the partial line; it is emitted only now.
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "OR app: three").unwrap();
    }
    let third = read_new_entries(&mut state, &path, &LogQuery::default()).unwrap();
    assert_eq!(third.len(), 1);
    assert_eq!(third[0].message, "three");
    assert_eq!(third[0].level, LogLevel::Error);
}

/// A shrunken file (rotation) resets the cursor.
#[test]
fn read_new_entries_resets_on_rotation() {
    let (_dir, path) = temp_file("rustasea.log", "2026-09-15T00:00:00Z INFO app: old line\n");
    let mut state = TailState::new(0);
    read_new_entries(&mut state, &path, &LogQuery::default()).unwrap();

    std::fs::write(&path, "2026-09-15T00:00:00Z INFO app: new\n").unwrap();
    let entries = read_new_entries(&mut state, &path, &LogQuery::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].message, "new");
}
