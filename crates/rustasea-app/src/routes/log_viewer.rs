//! Developer log-viewer surface — `/_logs` and `/_logs/json` (ADOPT-014).
//!
//! Both routes are **dev-only**: they are gated on [`AppState::debug`], so a
//! production build (debug `false`) answers `404` and never exposes the log
//! contents. The module itself is only compiled with the `log-viewer` feature,
//! so a default build has neither the routes nor the reader dependency edge.
//!
//! Parity target: `opcodesio/log-viewer` — a filterable, tail-able view of the
//! application log. This surface reads the same on-disk file the CLI
//! `log:show` command reads, through [`rustasea_logging`]'s reader module, so
//! both share one parsing/filtering implementation.
//!
//! The JSON endpoint is deliberately **fail-soft**: a missing file, an unknown
//! channel, or an I/O failure is reported as an `error` field alongside an empty
//! `entries` array with a `200` status — never a `500` — so the viewer renders a
//! clear message instead of a broken page.

use std::sync::Arc;

use axum::extract::{Extension, Query};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use serde::{Deserialize, Serialize};

use rustasea::http::AppState;
use rustasea::router::Router as RouteTable;
use rustasea::ConfigLoader;
use rustasea_logging::{
    read_entries, resolve_log_file, LogEntry, LogLevel, LogQuery, LoggingConfig,
};

/// The log-viewer HTML shell, served at `/_logs`.
///
/// Kept as a sibling asset so the markup is editable without touching Rust
/// code; `include_str!` bakes it into the binary at compile time.
const LOG_VIEWER_HTML: &str = include_str!("log_viewer.html");

/// Default maximum entries returned when the request omits `limit`.
const DEFAULT_LIMIT: usize = 200;

/// Maximum entries a request may ask for (a hard cap on the response size).
const MAX_LIMIT: usize = 2000;

/// Register the dev-only log-viewer routes onto `table`.
pub fn register(table: &mut RouteTable) {
    table.get_action("/_logs", log_viewer_ui);
    table.get_action("/_logs/json", log_viewer_json);
}

/// Query parameters accepted by `/_logs/json`.
#[derive(Debug, Default, Deserialize)]
struct LogViewerParams {
    /// Minimum severity (case-insensitive: `error`, `warn`, …).
    #[serde(default)]
    level: Option<String>,
    /// Case-insensitive substring over message, target and fields.
    #[serde(default)]
    grep: Option<String>,
    /// Maximum entries (the newest ones); capped at [`MAX_LIMIT`].
    #[serde(default)]
    limit: Option<usize>,
    /// Channel to read; defaults to the configured `[logging].default`.
    #[serde(default)]
    channel: Option<String>,
}

/// The JSON payload served by `/_logs/json`.
#[derive(Debug, Serialize)]
struct LogViewerPayload {
    /// Matching entries, newest last.
    entries: Vec<LogEntry>,
    /// Lines that could not be parsed.
    skipped: usize,
    /// The resolved file path (absent when resolution failed).
    file: Option<String>,
    /// A human-readable failure reason, present only on a soft failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl LogViewerPayload {
    /// An empty payload carrying a soft-failure reason.
    fn failure(reason: String) -> Self {
        Self {
            entries: Vec::new(),
            skipped: 0,
            file: None,
            error: Some(reason),
        }
    }
}

/// GET /_logs — serve the log-viewer shell (dev only).
///
/// The shell fetches `/_logs/json`; in production (`debug == false`) the route
/// answers `404` so neither the UI nor the log contents are reachable.
async fn log_viewer_ui(Extension(state): Extension<Arc<AppState>>) -> Response {
    if !state.debug {
        return StatusCode::NOT_FOUND.into_response();
    }
    Html(LOG_VIEWER_HTML).into_response()
}

/// GET /_logs/json — serve a filtered window of the log file (dev only).
///
/// Always answers `200` with a [`LogViewerPayload`] in debug; a resolution or
/// read failure is reported in the payload's `error` field rather than as a
/// `500`. In production (`debug == false`) the route answers `404`.
async fn log_viewer_json(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<LogViewerParams>,
) -> Response {
    if !state.debug {
        return StatusCode::NOT_FOUND.into_response();
    }
    axum::Json(build_payload(params)).into_response()
}

/// Resolve the log file and read the requested window, fail-soft.
///
/// Split from the handler so the resolution/read logic is unit-testable without
/// an axum request. Every failure becomes [`LogViewerPayload::failure`].
fn build_payload(params: LogViewerParams) -> LogViewerPayload {
    let config = load_config();
    let path = match resolve_log_file(&config, params.channel.as_deref()) {
        Ok(path) => path,
        Err(error) => return LogViewerPayload::failure(error.to_string()),
    };

    let query = LogQuery {
        min_level: params.level.as_deref().and_then(LogLevel::parse),
        since: None,
        grep: params.grep.clone().filter(|grep| !grep.is_empty()),
        limit: Some(params.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT)),
    };

    match read_entries(&path, &query) {
        Ok(result) => LogViewerPayload {
            entries: result.entries,
            skipped: result.skipped,
            file: Some(path.display().to_string()),
            error: None,
        },
        Err(error) => LogViewerPayload::failure(error.to_string()),
    }
}

/// Load `[logging]`, tolerating a missing or unreadable table.
///
/// Mirrors the CLI `log:show` resolution: a config that cannot be read at all
/// falls back to [`LoggingConfig::default`] so the viewer still resolves the
/// conventional default log path.
fn load_config() -> LoggingConfig {
    ConfigLoader::load_from(&["config/logging"])
        .ok()
        .and_then(|loader| LoggingConfig::from_loader(&loader).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An unknown channel fails soft: empty entries plus an error, never a panic.
    #[test]
    fn unknown_channel_is_a_soft_failure() {
        let payload = build_payload(LogViewerParams {
            channel: Some("definitely-not-a-channel".to_string()),
            ..Default::default()
        });
        assert!(payload.entries.is_empty());
        assert!(payload.file.is_none());
        assert!(payload.error.is_some());
    }

    /// An invalid level string is ignored (not a hard error).
    #[test]
    fn invalid_level_is_ignored() {
        let query = LogQuery {
            min_level: Some("not-a-level").as_deref().and_then(LogLevel::parse),
            ..Default::default()
        };
        assert!(query.min_level.is_none());
    }
}
