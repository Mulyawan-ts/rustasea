//! Log-file reading — parse, filter, resolve, and tail on-disk log files.
//!
//! This module is the read side of the logging facade. It complements
//! [`crate::init`] (the write side): where `init` installs a `tracing`
//! subscriber that appends formatted lines, `reader` turns those files back
//! into structured [`LogEntry`] values so a CLI command or a dev viewer can
//! filter and display them.
//!
//! The line grammar is documented on [`parse`]; this module adds the higher
//! level pieces:
//!
//! * [`LogQuery`] — level / time / substring / limit filtering.
//! * [`resolve_log_file`] — map a config channel to the file that backs it,
//!   transparently following the `daily`/`monthly` rotation naming.
//! * [`read_entries`] — read a file (bounded to its last 8 MiB), filter, limit.
//! * [`TailState`] + [`read_new_entries`] — an incremental reader for a growing
//!   file, used by the CLI `--follow` poll loop.
//!
//! Nothing here panics on malformed input: unparseable lines are counted and
//! skipped, and every I/O failure is a typed [`LoggingError`].

mod parse;

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::config::{LoggingConfig, DEFAULT_LOG_PATH};
use crate::error::{LoggingError, Result};

pub use parse::{parse_line, parse_lines, LogEntry, LogLevel};

/// Bytes read from the tail of a file when it is larger than this cap.
///
/// Log files can grow large; a viewer or CLI tail only ever wants the most
/// recent window, so reads are bounded to the final 8 MiB. The first (possibly
/// partial) line of a truncated read is dropped so a half-line never parses as
/// a corrupt entry.
pub const MAX_TAIL_BYTES: u64 = 8 * 1024 * 1024;

/// Filtering options applied to parsed entries.
///
/// Every field is optional; an unset field matches everything. Filters combine
/// with AND semantics via [`LogQuery::matches`].
#[derive(Debug, Clone, Default)]
pub struct LogQuery {
    /// Minimum severity (inclusive); entries below it are dropped.
    pub min_level: Option<LogLevel>,
    /// Lower timestamp bound (inclusive).
    pub since: Option<DateTime<Utc>>,
    /// Case-insensitive substring searched across message, target, and fields.
    pub grep: Option<String>,
    /// Maximum number of entries to keep (the most recent ones).
    pub limit: Option<usize>,
}

impl LogQuery {
    /// Whether `entry` passes every configured filter.
    ///
    /// `grep` is matched case-insensitively against the message, the target,
    /// and every field value and name.
    pub fn matches(&self, entry: &LogEntry) -> bool {
        if let Some(min_level) = self.min_level {
            if entry.level.rank() < min_level.rank() {
                return false;
            }
        }
        if let Some(since) = self.since {
            if entry.timestamp < since {
                return false;
            }
        }
        if let Some(grep) = self.grep.as_deref().filter(|g| !g.is_empty()) {
            let needle = grep.to_lowercase();
            if !entry_haystack(entry).contains(&needle) {
                return false;
            }
        }
        true
    }

    /// Apply `limit`, keeping the **most recent** entries.
    ///
    /// Limit is applied after filtering (so `--level=error --limit=10` yields the
    /// last ten errors, not the errors among the last ten lines). Because log
    /// consumers read forward and care about the newest events, the tail is kept
    /// rather than the head.
    fn apply_limit(&self, entries: &mut Vec<LogEntry>) {
        if let Some(limit) = self.limit {
            if entries.len() > limit {
                let start = entries.len() - limit;
                entries.drain(..start);
            }
        }
    }
}

/// Lowercased searchable text for one entry (message + target + fields).
fn entry_haystack(entry: &LogEntry) -> String {
    let mut hay = String::with_capacity(entry.message.len() + entry.target.len() + 32);
    hay.push_str(&entry.message);
    hay.push(' ');
    hay.push_str(&entry.target);
    for (key, value) in &entry.fields {
        hay.push(' ');
        hay.push_str(key);
        hay.push('=');
        hay.push_str(value);
    }
    hay.to_lowercase()
}

/// Outcome of a bounded file read.
#[derive(Debug, Clone, Default)]
pub struct LogReadResult {
    /// Entries that passed the query, newest last.
    pub entries: Vec<LogEntry>,
    /// Lines that could not be parsed.
    pub skipped: usize,
    /// Size of the file in bytes (the full file, not the read window).
    pub bytes: u64,
}

/// Resolve the on-disk log file for `channel` (or the config default).
///
/// The exact configured path is returned when it exists. When it does not — the
/// common case for a rolling `daily`/`monthly` channel, which writes
/// `stem-.YYYY-MM-DD.ext` and never the bare `stem.ext` — the newest rotated
/// sibling (lexicographically greatest, which for date-stamped names is the most
/// recent) is returned instead.
///
/// # Errors
///
/// [`LoggingError::UnknownChannel`] when `channel` (or the default) names no
/// configured channel, and [`LoggingError::LogFileMissing`] when neither the
/// exact path nor any rotated sibling exists.
pub fn resolve_log_file(config: &LoggingConfig, channel: Option<&str>) -> Result<PathBuf> {
    let name = channel.unwrap_or(&config.default);
    let channel_config = config.channel(name)?;
    let path = PathBuf::from(channel_config.path.as_deref().unwrap_or(DEFAULT_LOG_PATH));
    if path.is_file() {
        return Ok(path);
    }
    if let Some(rotated) = latest_rotated(&path) {
        return Ok(rotated);
    }
    Err(LoggingError::LogFileMissing {
        path: path.display().to_string(),
    })
}

/// Find the newest rotated sibling of `path`, if any.
///
/// Rotated names are `{stem}-.{date}.{ext}` (see
/// [`crate::init`](crate::init) and `tracing-appender`'s `join_date`), so a
/// sibling is any directory entry starting with `{stem}-` and ending with the
/// original extension. Date stamps are zero-padded and fixed-width, so a plain
/// string comparison picks the latest date.
fn latest_rotated(path: &Path) -> Option<PathBuf> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let stem = path.file_stem()?.to_str()?;
    let extension = path.extension().and_then(|ext| ext.to_str());
    let prefix = format!("{stem}-");
    let suffix = extension.map(|ext| format!(".{ext}"));

    let mut best: Option<(String, PathBuf)> = None;
    for entry in std::fs::read_dir(directory).ok()?.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !name.starts_with(&prefix) {
            continue;
        }
        if let Some(suffix) = &suffix {
            if !name.ends_with(suffix.as_str()) {
                continue;
            }
        }
        let is_newer = best
            .as_ref()
            .is_none_or(|(best_name, _)| name > best_name.as_str());
        if is_newer {
            best = Some((name.to_string(), entry.path()));
        }
    }
    best.map(|(_, path)| path)
}

/// Read `path`, parse its lines, and apply `query`.
///
/// At most the final [`MAX_TAIL_BYTES`] are read; the leading partial line of a
/// truncated window is discarded. Limit is applied last, keeping the newest
/// entries.
///
/// # Errors
///
/// [`LoggingError::Io`] when the file cannot be opened or read.
pub fn read_entries(path: &Path, query: &LogQuery) -> Result<LogReadResult> {
    let mut file =
        std::fs::File::open(path).map_err(|error| LoggingError::Io(error.to_string()))?;
    let bytes = file
        .metadata()
        .map(|metadata| metadata.len())
        .unwrap_or_default();

    let mut window = String::new();
    let truncated = bytes > MAX_TAIL_BYTES;
    if truncated {
        file.seek(SeekFrom::Start(bytes - MAX_TAIL_BYTES))
            .map_err(|error| LoggingError::Io(error.to_string()))?;
    }
    file.read_to_string(&mut window)
        .map_err(|error| LoggingError::Io(error.to_string()))?;
    if truncated {
        if let Some(newline) = window.find('\n') {
            window.drain(..=newline);
        }
    }

    let (parsed, skipped) = parse_lines(&window);
    let mut entries: Vec<LogEntry> = parsed
        .into_iter()
        .filter(|entry| query.matches(entry))
        .collect();
    query.apply_limit(&mut entries);

    Ok(LogReadResult {
        entries,
        skipped,
        bytes,
    })
}

/// Cursor for incremental tailing of a growing file.
///
/// Holds the byte offset already consumed plus the trailing partial line (a
/// write may be observed mid-line). [`read_new_entries`] advances both.
#[derive(Debug, Clone, Default)]
pub struct TailState {
    offset: u64,
    carry: String,
}

impl TailState {
    /// Create a cursor positioned at `offset` with no partial-line carry.
    pub fn new(offset: u64) -> Self {
        Self {
            offset,
            carry: String::new(),
        }
    }

    /// The current byte offset (start of the unread region).
    pub fn offset(&self) -> u64 {
        self.offset
    }
}

/// Read every complete new line appended to `path` since `state`'s offset.
///
/// Newly read bytes are prepended with the carry from the previous call and
/// split on `\n`; the trailing fragment (if any) becomes the new carry, so a
/// line is only emitted once it is complete. When the file has shrunk — a
/// rotation replaced it — the cursor resets to the start.
///
/// # Errors
///
/// [`LoggingError::Io`] when the file cannot be opened or read.
pub fn read_new_entries(
    state: &mut TailState,
    path: &Path,
    query: &LogQuery,
) -> Result<Vec<LogEntry>> {
    let mut file =
        std::fs::File::open(path).map_err(|error| LoggingError::Io(error.to_string()))?;
    let length = file
        .metadata()
        .map(|metadata| metadata.len())
        .unwrap_or_default();
    if length < state.offset {
        // The file was truncated or rotated: restart from the beginning.
        state.offset = 0;
        state.carry.clear();
    }
    file.seek(SeekFrom::Start(state.offset))
        .map_err(|error| LoggingError::Io(error.to_string()))?;

    let mut chunk = String::new();
    file.read_to_string(&mut chunk)
        .map_err(|error| LoggingError::Io(error.to_string()))?;
    state.offset += chunk.len() as u64;

    let mut buffer = std::mem::take(&mut state.carry);
    buffer.push_str(&chunk);
    let mut entries = Vec::new();
    let mut parts = buffer.split('\n');
    // The final fragment is either the trailing partial line or "" (when the
    // chunk ended exactly on a newline); either way it is not a complete line.
    if let Some(tail) = parts.next_back() {
        state.carry = tail.to_string();
    }
    for line in parts {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(entry) = parse_line(line) {
            if query.matches(&entry) {
                entries.push(entry);
            }
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests;
