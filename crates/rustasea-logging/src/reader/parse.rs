//! Log-line parsing — the plain-text `tracing` `fmt` grammar.
//!
//! RustaSea writes its log files with `tracing_subscriber::fmt`'s default
//! (plain-text) formatter (see [`crate::init`]): no JSON, ANSI disabled on file
//! channels. Each line follows:
//!
//! ```text
//! TIMESTAMP(UTC RFC3339 'Z') SP LEVEL TARGET: MESSAGE key=value ...
//! ```
//!
//! `LEVEL` is right-aligned in five columns (` WARN`, `DEBUG`), `TARGET` is the
//! tracing target up to the first `": "`, and the trailing `key=value` fields
//! are `tracing` field records. Field values are written with `Display` when
//! they contain no special characters (so a value may contain unquoted spaces,
//! e.g. `error=broadcast connection pusher is not configured`) and with `Debug`
//! otherwise (quoted, with `\"`/`\n`/`\t` escapes).
//!
//! The parser is deliberately tolerant: a malformed or partial line yields
//! `None` and the caller counts it, rather than failing the whole read.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Severity of a parsed log entry.
///
/// Ordered from least to most severe so [`LogLevel::rank`] and the derived
/// [`Ord`] agree; serde renders the canonical uppercase token (`"ERROR"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum LogLevel {
    /// `TRACE`.
    Trace,
    /// `DEBUG`.
    Debug,
    /// `INFO`.
    Info,
    /// `WARN`.
    Warn,
    /// `ERROR`.
    Error,
}

impl LogLevel {
    /// Parse a `tracing` level token (case-insensitive, whitespace-trimmed).
    ///
    /// Returns `None` for anything that is not one of the five standard
    /// levels — including tracing's `OFF`, which is never emitted to a file.
    pub fn parse(token: &str) -> Option<Self> {
        match token.trim().to_ascii_uppercase().as_str() {
            "TRACE" => Some(LogLevel::Trace),
            "DEBUG" => Some(LogLevel::Debug),
            "INFO" => Some(LogLevel::Info),
            "WARN" => Some(LogLevel::Warn),
            "ERROR" => Some(LogLevel::Error),
            _ => None,
        }
    }

    /// Canonical uppercase token.
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }

    /// Numeric severity rank (TRACE = 0 … ERROR = 4).
    ///
    /// Used by the query's `min_level` filter: an entry passes when its rank is
    /// greater than or equal to the requested minimum.
    pub fn rank(self) -> u8 {
        match self {
            LogLevel::Trace => 0,
            LogLevel::Debug => 1,
            LogLevel::Info => 2,
            LogLevel::Warn => 3,
            LogLevel::Error => 4,
        }
    }
}

impl std::fmt::Display for LogLevel {
    /// Renders the canonical uppercase token.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One parsed log line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    /// Event timestamp (UTC), parsed from the RFC 3339 `Z` prefix.
    pub timestamp: DateTime<Utc>,
    /// Event severity.
    pub level: LogLevel,
    /// Tracing target (module path), up to the first `": "`.
    pub target: String,
    /// The event's message (may be empty when the event carried only fields).
    pub message: String,
    /// Trailing `key=value` fields, in source order.
    pub fields: Vec<(String, String)>,
}

/// Parse a single log line, returning `None` for malformed or partial input.
///
/// ANSI escape sequences are stripped first (files carry none, but a captured
/// TTY stream may), CRLF line endings are tolerated, and a blank line yields
/// `None`.
pub fn parse_line(line: &str) -> Option<LogEntry> {
    let cleaned = strip_ansi(line);
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return None;
    }

    // 1. Timestamp — the first whitespace-delimited token.
    let (timestamp_token, rest) = split_first_token(cleaned)?;
    let timestamp = DateTime::parse_from_rfc3339(timestamp_token)
        .ok()?
        .with_timezone(&Utc);

    // 2. Level — the next token (tracing right-pads it, so trimming matters).
    let (level_token, rest) = split_first_token(rest)?;
    let level = LogLevel::parse(level_token)?;

    // 3. Target — everything up to the first `": "` (a target such as
    //    `sqlx::query` contains colons but never a colon-space, so this is a
    //    safe delimiter).
    let (target, remainder) = match rest.find(": ") {
        Some(index) => (rest[..index].trim(), &rest[index + 2..]),
        None => (rest.trim(), ""),
    };
    if target.is_empty() {
        return None;
    }

    // 4. Message + trailing fields.
    let (message, fields) = split_message_fields(remainder);

    Some(LogEntry {
        timestamp,
        level,
        target: target.to_string(),
        message,
        fields,
    })
}

/// Parse every line of `text`, returning `(entries, skipped)`.
///
/// Blank lines are ignored without counting as skipped; any other line that
/// fails to parse increments the skipped counter.
pub fn parse_lines(text: &str) -> (Vec<LogEntry>, usize) {
    let mut entries = Vec::new();
    let mut skipped = 0usize;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match parse_line(line) {
            Some(entry) => entries.push(entry),
            None => skipped += 1,
        }
    }
    (entries, skipped)
}

/// Split `input` into its first whitespace-delimited token and the remainder.
///
/// Leading whitespace is skipped; the remainder keeps its own leading
/// whitespace so callers can rely on a single trailing trim.
fn split_first_token(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.find(char::is_whitespace) {
        Some(index) => Some((&trimmed[..index], &trimmed[index..])),
        None => Some((trimmed, "")),
    }
}

/// Split a `MESSAGE key=value …` remainder into the message and the fields.
///
/// The message is every leading token that is not a field start; a field starts
/// at the first `identifier=` boundary (preceded by start-of-input or
/// whitespace). Text with no `=` therefore stays part of the message.
fn split_message_fields(remainder: &str) -> (String, Vec<(String, String)>) {
    match find_field_start(remainder) {
        Some(index) => {
            let message = remainder[..index].trim().to_string();
            (message, parse_fields(&remainder[index..]))
        }
        None => (remainder.trim().to_string(), Vec::new()),
    }
}

/// Find the byte index where the first `identifier=` field begins, if any.
fn find_field_start(input: &str) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let starts_word = index == 0 || bytes[index - 1].is_ascii_whitespace();
        if starts_word && is_ident_start(bytes[index]) {
            let mut end = index;
            while end < bytes.len() && is_ident_char(bytes[end]) {
                end += 1;
            }
            if end < bytes.len() && bytes[end] == b'=' {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

/// Parse the `key=value` field run starting at `input`.
///
/// Each value is either a quoted `Debug` string (unescaped) or an unquoted run
/// that ends at the next `identifier=` boundary.
fn parse_fields(input: &str) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    let mut rest = input.trim_start();
    while !rest.is_empty() {
        let Some(eq) = rest.find('=') else {
            break;
        };
        let key = &rest[..eq];
        if key.is_empty() || !key.bytes().all(is_ident_char) {
            break;
        }
        let after = &rest[eq + 1..];
        let (value, remainder) = if after.starts_with('"') {
            read_quoted(after)
        } else {
            read_unquoted(after)
        };
        fields.push((key.to_string(), value));
        rest = remainder.trim_start();
    }
    fields
}

/// Read an unquoted field value, stopping at the next `identifier=` boundary.
fn read_unquoted(input: &str) -> (String, &str) {
    match find_field_start(input) {
        Some(index) => (input[..index].trim().to_string(), &input[index..]),
        None => (input.trim().to_string(), ""),
    }
}

/// Read a double-quoted value starting at `input`, honouring basic escapes.
///
/// The opening quote must already be present. A missing closing quote consumes
/// the remainder of the input.
fn read_quoted(input: &str) -> (String, &str) {
    let mut out = String::new();
    let mut chars = input.char_indices();
    chars.next(); // consume the opening quote
    let mut end = input.len();
    while let Some((index, ch)) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some((_, 'n')) => out.push('\n'),
                Some((_, 't')) => out.push('\t'),
                Some((_, 'r')) => out.push('\r'),
                Some((_, '0')) => out.push('\0'),
                Some((_, '\\')) => out.push('\\'),
                Some((_, '"')) => out.push('"'),
                Some((_, other)) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            },
            '"' => {
                end = index + 1;
                break;
            }
            other => out.push(other),
        }
    }
    (out, &input[end..])
}

/// Whether `byte` can start an identifier (ASCII letter or `_`).
fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

/// Whether `byte` can continue an identifier (ASCII alphanumeric, `_` or `.`).
///
/// The `.` accommodates nested tracing field names such as `db.statement`.
fn is_ident_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.'
}

/// Strip ANSI CSI escape sequences (`ESC [ … final`).
fn strip_ansi(input: &str) -> String {
    if !input.contains('\u{1b}') {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for tail in chars.by_ref() {
                    if ('@'..='~').contains(&tail) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain `WARN` line parses its timestamp, level, target, message, and
    /// the unquoted field whose value itself contains spaces.
    #[test]
    fn parses_warn_with_unquoted_spaced_value() {
        let line = "2026-09-15T19:52:20.635932Z  WARN rustasea_app::bootstrap::app: \
                    broadcast manager build failed error=broadcast connection pusher is not configured";
        let entry = parse_line(line).expect("valid line");
        assert_eq!(entry.level, LogLevel::Warn);
        assert_eq!(entry.target, "rustasea_app::bootstrap::app");
        assert_eq!(entry.message, "broadcast manager build failed");
        assert_eq!(
            entry.fields,
            vec![(
                "error".to_string(),
                "broadcast connection pusher is not configured".to_string()
            )]
        );
        assert_eq!(
            entry.timestamp.to_rfc3339(),
            "2026-09-15T19:52:20.635932+00:00"
        );
    }

    /// A `DEBUG` line with a `sqlx`-style quoted value and multiple fields.
    #[test]
    fn parses_quoted_values_and_multiple_fields() {
        let line = "2026-09-15T19:55:17.831686Z DEBUG sqlx::query: summary=\"INSERT INTO \
                    failed_jobs (id, \\\"x\\\")\" rows_affected=1 rows_returned=0";
        let entry = parse_line(line).expect("valid line");
        assert_eq!(entry.level, LogLevel::Debug);
        assert_eq!(entry.target, "sqlx::query");
        assert_eq!(entry.message, "");
        assert_eq!(entry.fields[0].0, "summary");
        assert_eq!(entry.fields[0].1, "INSERT INTO failed_jobs (id, \"x\")");
        assert_eq!(
            entry.fields[1],
            ("rows_affected".to_string(), "1".to_string())
        );
        assert_eq!(
            entry.fields[2],
            ("rows_returned".to_string(), "0".to_string())
        );
    }

    /// Escapes inside a quoted value are decoded (`\n`, `\t`, `\\`).
    #[test]
    fn decodes_quoted_escapes() {
        let line = "2026-01-01T00:00:00Z INFO app: db.statement=\"a\\nb\\tc\\\\d\"";
        let entry = parse_line(line).expect("valid line");
        assert_eq!(entry.fields[0].1, "a\nb\tc\\d");
    }

    /// A message-only line carries no fields.
    #[test]
    fn parses_message_without_fields() {
        let entry = parse_line("2026-01-01T00:00:00Z INFO app::boot: server started").unwrap();
        assert_eq!(entry.message, "server started");
        assert!(entry.fields.is_empty());
    }

    /// Malformed and partial lines yield `None`.
    #[test]
    fn rejects_malformed_lines() {
        assert!(parse_line("").is_none());
        assert!(parse_line("   ").is_none());
        assert!(parse_line("not-a-timestamp INFO app: hi").is_none());
        assert!(parse_line("2026-01-01T00:00:00Z BOGUS app: hi").is_none());
        assert!(parse_line("2026-01-01T00:00:00Z").is_none());
        assert!(parse_line("2026-01-01T00:00:00Z INFO ").is_none());
    }

    /// ANSI sequences are stripped and CRLF is tolerated.
    #[test]
    fn strips_ansi_and_tolerates_crlf() {
        let line = "\u{1b}[32m2026-01-01T00:00:00Z INFO app: hi\u{1b}[0m\r";
        let entry = parse_line(line).expect("valid line");
        assert_eq!(entry.message, "hi");
        assert_eq!(entry.level, LogLevel::Info);
    }

    /// `parse_lines` counts malformed lines and ignores blank ones.
    #[test]
    fn parse_lines_counts_skipped() {
        let text =
            "2026-01-01T00:00:00Z INFO app: one\n\ngarbage\n2026-01-01T00:00:01Z WARN app: two\n";
        let (entries, skipped) = parse_lines(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(skipped, 1);
    }

    /// Level tokens parse case-insensitively and rank monotonically.
    #[test]
    fn level_parse_and_rank() {
        assert_eq!(LogLevel::parse(" warn "), Some(LogLevel::Warn));
        assert_eq!(LogLevel::parse("Error"), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse("OFF"), None);
        assert!(LogLevel::Trace.rank() < LogLevel::Error.rank());
        assert_eq!(LogLevel::Error.as_str(), "ERROR");
    }
}
