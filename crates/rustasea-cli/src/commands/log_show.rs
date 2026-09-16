//! `log:show` — filter, print, and tail the application log file.
//!
//! Reads the on-disk log file that backs a configured logging channel through
//! [`rustasea_logging`]'s reader module, applies the requested filters, and
//! prints the matching entries as a table (default) or as newline-delimited
//! JSON (`--json`, ideal for piping into `jq`).
//!
//! ```text
//! log:show [--level=error] [--channel=daily] [--since=2026-09-15T00:00:00Z]
//!          [--grep=text] [--limit=200] [--follow] [--json]
//! ```
//!
//! * `--level`  — minimum severity (inclusive).
//! * `--channel` — which `[logging.channels.*]` entry to read; defaults to the
//!   configured `default` channel.
//! * `--since`  — RFC 3339 lower timestamp bound (inclusive).
//! * `--grep`   — case-insensitive substring over message, target and fields.
//! * `--limit`  — maximum entries to print (the **newest** ones; default 200).
//! * `--follow` — after printing the current selection, poll for appended lines
//!   every 250 ms until `Ctrl-C`.
//! * `--json`   — one JSON object per line instead of a table.
//!
//! The log file is resolved from `config/logging.toml`; a `daily`/`monthly`
//! channel transparently follows the newest rotated sibling when the bare path
//! does not exist.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};

use rustasea_logging::{
    read_entries, read_new_entries, resolve_log_file, LogEntry, LogLevel, LogQuery, LoggingConfig,
    TailState,
};

use crate::artisan::{Command, CommandOutput, Io};
use crate::error::{CliError, CliResult};
use crate::output;

/// Poll interval for `--follow`.
const FOLLOW_INTERVAL: Duration = Duration::from_millis(250);

/// Default number of entries printed when `--limit` is omitted.
const DEFAULT_LIMIT: usize = 200;

/// Parsed `log:show` options.
#[derive(Debug, Default)]
struct Options {
    /// Minimum severity (`--level`).
    level: Option<LogLevel>,
    /// Channel name (`--channel`).
    channel: Option<String>,
    /// Lower timestamp bound (`--since`).
    since: Option<DateTime<Utc>>,
    /// Case-insensitive substring (`--grep`).
    grep: Option<String>,
    /// Maximum entries (`--limit`).
    limit: Option<usize>,
    /// Whether to follow appended lines (`--follow`).
    follow: bool,
    /// Whether to emit newline-delimited JSON (`--json`).
    json: bool,
}

/// `log:show` — filter, print, and optionally tail the application log.
pub struct LogShow;

#[async_trait]
impl Command for LogShow {
    /// Command signature.
    fn signature(&self) -> &'static str {
        "log:show"
    }

    /// Usage line rendered by `list`.
    fn usage(&self) -> Option<&'static str> {
        Some(
            "log:show [--level=error] [--channel=daily] \
             [--since=2026-09-15T00:00:00Z] [--grep=text] [--limit=200] [--follow] [--json]",
        )
    }

    /// One-line help rendered by `list`.
    fn help(&self) -> Option<&'static str> {
        Some("Show (and optionally follow) the application log file")
    }

    /// Execute: resolve the file, print the selection, and follow if asked.
    async fn run(&self, args: Vec<String>, io: &mut Io) -> CliResult<()> {
        let options = parse_args(&args)?;
        let config = load_config();
        let path = resolve_path(&config, options.channel.as_deref())?;
        let query = LogQuery {
            min_level: options.level,
            since: options.since,
            grep: options.grep.clone(),
            limit: Some(options.limit.unwrap_or(DEFAULT_LIMIT)),
        };

        let result = read_entries(&path, &query).map_err(map_read_error)?;

        if options.follow {
            // Stream the backlog and every new line straight to stdout: a
            // terminal sees output as it arrives, and nothing accumulates in
            // the in-memory `Io` buffer that `Artisan::call` would otherwise
            // hold until the follow loop ends. A broken pipe ends cleanly.
            let mut sink = StreamSink::new(std::io::stdout());
            if let Err(error) = render_entries(&result.entries, options.json, &mut sink) {
                return stop_on_broken_pipe(error);
            }
            // Seed the cursor from the byte count `read_entries` already
            // reported, so lines appended between the two calls are not lost
            // (and no second metadata syscall is needed).
            let mut follower = Follower::new(path, result.bytes, options.channel.clone());
            // The follow stream is unbounded, so it ignores the initial limit.
            let follow_query = LogQuery {
                limit: None,
                ..query
            };
            follow(
                &config,
                &mut follower,
                &follow_query,
                options.json,
                &mut sink,
            )
            .await?;
        } else {
            render_entries(&result.entries, options.json, io)?;
        }
        Ok(())
    }
}

/// Parse the raw `log:show` argument vector.
///
/// Supports `--flag=value` and `--flag value` for the valued flags, plus the
/// `--follow` / `--json` toggles. Any other flag is a typed
/// [`CliError::InvalidArguments`].
fn parse_args(args: &[String]) -> CliResult<Options> {
    let invalid = |detail: String| CliError::InvalidArguments {
        command: "log:show".to_string(),
        detail,
    };

    let mut options = Options::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--follow" {
            options.follow = true;
        } else if arg == "--json" {
            options.json = true;
        } else if arg == "--level" {
            let value = next_value(&mut iter, "--level", &invalid)?;
            options.level = Some(parse_level(&value, &invalid)?);
        } else if let Some(value) = arg.strip_prefix("--level=") {
            options.level = Some(parse_level(value, &invalid)?);
        } else if arg == "--channel" {
            options.channel = Some(next_value(&mut iter, "--channel", &invalid)?);
        } else if let Some(value) = arg.strip_prefix("--channel=") {
            options.channel = Some(non_empty("--channel", value, &invalid)?);
        } else if arg == "--since" {
            let value = next_value(&mut iter, "--since", &invalid)?;
            options.since = Some(parse_since(&value, &invalid)?);
        } else if let Some(value) = arg.strip_prefix("--since=") {
            options.since = Some(parse_since(value, &invalid)?);
        } else if arg == "--grep" {
            options.grep = Some(next_value(&mut iter, "--grep", &invalid)?);
        } else if let Some(value) = arg.strip_prefix("--grep=") {
            options.grep = Some(value.to_string());
        } else if arg == "--limit" {
            let value = next_value(&mut iter, "--limit", &invalid)?;
            options.limit = Some(parse_limit(&value, &invalid)?);
        } else if let Some(value) = arg.strip_prefix("--limit=") {
            options.limit = Some(parse_limit(value, &invalid)?);
        } else if arg.starts_with('-') {
            return Err(invalid(format!("unrecognized flag `{arg}`")));
        }
    }
    Ok(options)
}

/// Pull the value that follows a valued flag.
fn next_value(
    iter: &mut std::slice::Iter<'_, String>,
    flag: &str,
    invalid: &impl Fn(String) -> CliError,
) -> CliResult<String> {
    let value = iter
        .next()
        .ok_or_else(|| invalid(format!("`{flag}` requires a value")))?;
    non_empty(flag, value, invalid)
}

/// Validate that a flag value is non-empty.
fn non_empty(flag: &str, value: &str, invalid: &impl Fn(String) -> CliError) -> CliResult<String> {
    if value.is_empty() {
        return Err(invalid(format!("`{flag}` requires a non-empty value")));
    }
    Ok(value.to_string())
}

/// Parse a level token into a [`LogLevel`].
fn parse_level(value: &str, invalid: &impl Fn(String) -> CliError) -> CliResult<LogLevel> {
    LogLevel::parse(value).ok_or_else(|| {
        invalid(format!(
            "invalid level `{value}` (expected trace/debug/info/warn/error)"
        ))
    })
}

/// Parse an RFC 3339 timestamp into a UTC [`DateTime`].
fn parse_since(value: &str, invalid: &impl Fn(String) -> CliError) -> CliResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|_| {
            invalid(format!(
                "invalid `--since` timestamp `{value}` (expected RFC 3339)"
            ))
        })
}

/// Parse a non-negative entry limit.
fn parse_limit(value: &str, invalid: &impl Fn(String) -> CliError) -> CliResult<usize> {
    value
        .parse::<usize>()
        .map_err(|_| invalid(format!("invalid `--limit` value `{value}`")))
}

/// Load `[logging]`, tolerating a missing or unreadable table.
///
/// Mirrors [`LoggingConfig::from_loader`]'s missing-table policy: a config that
/// cannot be read at all falls back to [`LoggingConfig::default`] so the
/// command still resolves the conventional [`rustasea_logging::DEFAULT_LOG_PATH`].
fn load_config() -> LoggingConfig {
    rustasea_config::ConfigLoader::load_from(&["config/logging"])
        .ok()
        .and_then(|loader| LoggingConfig::from_loader(&loader).ok())
        .unwrap_or_default()
}

/// Resolve the log file for `channel`, mapping a missing file to a clear error.
fn resolve_path(config: &LoggingConfig, channel: Option<&str>) -> CliResult<PathBuf> {
    match resolve_log_file(config, channel) {
        Ok(path) => Ok(path),
        Err(rustasea_logging::LoggingError::LogFileMissing { path }) => {
            let name = channel.unwrap_or(&config.default);
            Err(CliError::Domain(format!(
                "log file not found: {path} (channel '{name}')"
            )))
        }
        Err(error) => Err(CliError::Domain(error.to_string())),
    }
}

/// Destination for rendered log lines.
///
/// The command renders through this abstraction so the buffered and streaming
/// paths share one renderer: [`Io`] accumulates into the in-process buffer that
/// `Artisan::call` returns, while [`StreamSink`] writes straight to a terminal.
/// The streaming path is what keeps a long `--follow` session's memory bounded.
trait LineSink {
    /// Emit one rendered line (a trailing newline is added by the sink).
    fn line(&mut self, text: &str) -> CliResult<()>;
}

impl LineSink for Io {
    /// Append to the in-process buffer (used by the non-follow path).
    fn line(&mut self, text: &str) -> CliResult<()> {
        CommandOutput::line(self, text);
        Ok(())
    }
}

/// Streaming sink: writes each line straight through and flushes per line.
///
/// Used by `--follow` so the terminal sees the backlog and every new event as
/// it arrives — nothing is held in the unbounded in-process buffer.
struct StreamSink<W: Write> {
    /// Underlying writer (normally `std::io::stdout()`).
    writer: W,
}

impl<W: Write> StreamSink<W> {
    /// Wrap `writer` in a flushing line sink.
    fn new(writer: W) -> Self {
        Self { writer }
    }
}

impl<W: Write> LineSink for StreamSink<W> {
    /// Write one line and flush so it appears immediately.
    fn line(&mut self, text: &str) -> CliResult<()> {
        writeln!(self.writer, "{text}")?;
        self.writer.flush()?;
        Ok(())
    }
}

/// Whether `error` is a broken-pipe I/O failure (the reader went away).
///
/// A `log:show --follow | head` pipe closes early; that is a clean end, not a
/// command failure, so the caller returns `Ok(())`.
fn is_broken_pipe(error: &CliError) -> bool {
    matches!(error, CliError::Io(io) if io.kind() == std::io::ErrorKind::BrokenPipe)
}

/// Render a batch of entries as a table or newline-delimited JSON.
fn render_entries(entries: &[LogEntry], json: bool, sink: &mut impl LineSink) -> CliResult<()> {
    if json {
        for entry in entries {
            sink.line(&serde_json::to_string(entry)?)?;
        }
        return Ok(());
    }

    let mut rows = vec![vec![
        "TIMESTAMP".to_string(),
        "LEVEL".to_string(),
        "TARGET".to_string(),
        "MESSAGE".to_string(),
    ]];
    for entry in entries {
        rows.push(vec![
            format_timestamp(entry.timestamp),
            entry.level.as_str().to_string(),
            entry.target.clone(),
            format_message(entry),
        ]);
    }
    sink.line(output::table(rows).trim_end())
}

/// Format a timestamp the way the log file does (`…Z`, microsecond precision).
fn format_timestamp(timestamp: DateTime<Utc>) -> String {
    timestamp.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Render an entry's message with its fields appended as `key=value`.
fn format_message(entry: &LogEntry) -> String {
    let mut out = entry.message.clone();
    for (key, value) in &entry.fields {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(key);
        out.push('=');
        out.push_str(value);
    }
    out
}

/// Rotation-aware tail cursor for `--follow`.
///
/// Holds the currently tailed path plus its incremental [`TailState`]. Each
/// [`Follower::tick`] re-resolves the channel's file (a `read_dir` at worst) so
/// a `daily`/`monthly` rollover to a newer dated sibling is picked up: when the
/// resolved path changes, any trailing lines in the old file are drained first,
/// then the cursor restarts at offset 0 for the new file.
struct Follower {
    /// Path currently being tailed.
    path: PathBuf,
    /// Incremental read cursor for `path`.
    state: TailState,
    /// Channel selector re-resolved each tick (`None` = config default).
    channel: Option<String>,
}

impl Follower {
    /// Start following `path` from byte `offset` for channel `channel`.
    fn new(path: PathBuf, offset: u64, channel: Option<String>) -> Self {
        Self {
            path,
            state: TailState::new(offset),
            channel,
        }
    }

    /// Perform one follow tick: re-resolve for rotation, then drain new lines.
    ///
    /// Returns the number of entries emitted (old-file drain + new-file lines).
    fn tick(
        &mut self,
        config: &LoggingConfig,
        query: &LogQuery,
        json: bool,
        sink: &mut impl LineSink,
    ) -> CliResult<usize> {
        // Re-resolve every tick. A transient error (the file is momentarily
        // absent mid-rotation) is ignored: keep the current path and retry.
        if let Ok(resolved) = resolve_log_file(config, self.channel.as_deref()) {
            if resolved != self.path {
                // Rotation: drain whatever the old file still holds (best
                // effort — it may already have been removed) before switching.
                let drained = self.tail_current(query, json, sink).unwrap_or(0);
                self.path = resolved;
                self.state = TailState::new(0);
                let fresh = self.tail_current(query, json, sink)?;
                return Ok(drained + fresh);
            }
        }
        self.tail_current(query, json, sink)
    }

    /// Read and emit the lines appended to the current path since its cursor.
    fn tail_current(
        &mut self,
        query: &LogQuery,
        json: bool,
        sink: &mut impl LineSink,
    ) -> CliResult<usize> {
        let entries =
            read_new_entries(&mut self.state, &self.path, query).map_err(map_read_error)?;
        render_entries(&entries, json, sink)?;
        Ok(entries.len())
    }
}

/// Poll the follower until `Ctrl-C`, streaming each batch to `sink`.
///
/// Every [`FOLLOW_INTERVAL`] the follower re-resolves its file (following
/// rotation) and drains new lines; a broken pipe (`… --follow | head`) ends the
/// loop cleanly.
async fn follow(
    config: &LoggingConfig,
    follower: &mut Follower,
    query: &LogQuery,
    json: bool,
    sink: &mut impl LineSink,
) -> CliResult<()> {
    let mut ticker = tokio::time::interval(FOLLOW_INTERVAL);
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            _ = ticker.tick() => {
                match follower.tick(config, query, json, sink) {
                    Ok(_) => {}
                    Err(error) if is_broken_pipe(&error) => return Ok(()),
                    Err(error) => return Err(error),
                }
            }
        }
    }
}

/// Map a reader I/O failure onto a typed CLI domain error.
fn map_read_error(error: rustasea_logging::LoggingError) -> CliError {
    CliError::Domain(error.to_string())
}

/// Return `Ok(())` for a broken-pipe render failure, otherwise propagate it.
fn stop_on_broken_pipe(error: CliError) -> CliResult<()> {
    if is_broken_pipe(&error) {
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(test)]
mod tests;
