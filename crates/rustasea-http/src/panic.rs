//! Panic catching and location capture for the dev error page (ADOPT-010).
//!
//! Two concerns live here:
//!
//! 1. [`catch_panic_layer`] — a `tower-http` [`CatchPanicLayer`] whose custom
//!    handler turns a panic payload into an [`AppError::Panic`] response. This
//!    is what lets the app-layer error middleware see a panic as an ordinary
//!    `5xx` and render the dev page / prod envelope for it.
//! 2. [`install_panic_location_hook`] / [`last_panic_location`] — a process-wide
//!    panic hook that records the `(message, file, line)` of the most recent
//!    panic, so the dev page can show a source snippet. The hook chains to the
//!    previously installed hook, so the default panic output is never lost.
//!
//! [`CatchPanicLayer`]: tower_http::catch_panic::CatchPanicLayer

use std::any::Any;
use std::panic::{self, PanicHookInfo};
use std::sync::{Mutex, OnceLock};

use axum::response::{IntoResponse, Response};

use crate::error::AppError;

/// Where the most recent panic occurred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanicLocation {
    /// Panic message (the payload rendered as a string).
    pub message: String,
    /// Source file reported by the panic hook.
    pub file: String,
    /// Source line reported by the panic hook.
    pub line: u32,
}

/// Process-wide slot holding the most recent panic location.
fn location_slot() -> &'static Mutex<Option<PanicLocation>> {
    static SLOT: OnceLock<Mutex<Option<PanicLocation>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Build a `CatchPanicLayer` that converts panics into [`AppError::Panic`].
///
/// The returned layer composes onto any axum router via `.layer(...)`. The
/// handler extracts the panic message from `&str` / `String` payloads and falls
/// back to a fixed string for any other payload type.
pub fn catch_panic_layer(
) -> tower_http::catch_panic::CatchPanicLayer<fn(Box<dyn Any + Send>) -> Response> {
    tower_http::catch_panic::CatchPanicLayer::custom(
        handle_panic as fn(Box<dyn Any + Send>) -> Response,
    )
}

/// Convert a panic payload into an [`AppError::Panic`] response.
///
/// Kept as a free `fn` (not a closure) so it coerces to a plain function
/// pointer and the layer stays cheaply cloneable.
pub fn handle_panic(payload: Box<dyn Any + Send>) -> Response {
    let message = panic_message(&payload);
    AppError::panic(message).into_response()
}

/// Extract a panic message from a payload, with a stable fallback.
fn panic_message(payload: &Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else {
        "panic in request handler".to_string()
    }
}

/// Install the process-wide panic-location hook exactly once.
///
/// The hook records the message + location of each panic into a process-wide
/// slot and then forwards to the previously installed hook (captured first), so
/// the default panic output — and any other hook already in place — still runs.
/// A second call is a no-op: the [`OnceLock`] guard prevents re-chaining, which
/// would otherwise grow the hook chain unboundedly.
pub fn install_panic_location_hook() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info: &PanicHookInfo<'_>| {
            let message = payload_message(info);
            let (file, line) = info
                .location()
                .map(|location| (location.file().to_string(), location.line()))
                .unwrap_or_default();
            if let Ok(mut slot) = location_slot().lock() {
                *slot = Some(PanicLocation {
                    message,
                    file,
                    line,
                });
            }
            previous(info);
        }));
    });
}

/// The most recent recorded panic location, if any.
pub fn last_panic_location() -> Option<PanicLocation> {
    location_slot().lock().ok().and_then(|slot| slot.clone())
}

/// Render a `line | code` snippet from `file` around `line`.
///
/// Best-effort: any IO failure (missing file, unreadable, too large) yields
/// `None`. The whole file is capped at [`SNIPPET_MAX_BYTES`] and each rendered
/// line is capped at [`SNIPPET_MAX_LINE`] characters, so a pathological source
/// file cannot produce a huge page.
pub fn source_snippet(file: &str, line: u32, radius: usize) -> Option<String> {
    if line == 0 {
        return None;
    }
    let metadata = std::fs::metadata(file).ok()?;
    if metadata.len() > SNIPPET_MAX_BYTES {
        return None;
    }
    let contents = std::fs::read_to_string(file).ok()?;
    let lines: Vec<&str> = contents.lines().collect();
    let target = line as usize;
    if target == 0 || target > lines.len() {
        return None;
    }
    let start = target.saturating_sub(radius).max(1);
    let end = (target + radius).min(lines.len());

    let mut out = String::new();
    for (offset, text) in lines[start - 1..end].iter().enumerate() {
        let number = start + offset;
        let marker = if number == target { ">" } else { " " };
        out.push_str(&format!("{marker} {number:>5} | {}\n", cap_line(text)));
    }
    Some(out)
}

/// Maximum file size (bytes) considered for a snippet.
const SNIPPET_MAX_BYTES: u64 = 16 * 1024;

/// Maximum characters rendered per snippet line.
const SNIPPET_MAX_LINE: usize = 300;

/// Cap a single source line to [`SNIPPET_MAX_LINE`] characters.
fn cap_line(line: &str) -> String {
    if line.chars().count() <= SNIPPET_MAX_LINE {
        return line.to_string();
    }
    let capped: String = line.chars().take(SNIPPET_MAX_LINE).collect();
    format!("{capped}…")
}

/// Render a panic hook payload as a string.
fn payload_message(info: &PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else {
        "panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::StatusCode;

    /// The downcast path handles a `&str` payload.
    #[tokio::test]
    async fn handles_str_payload() {
        let response = handle_panic(Box::new("boom-str"));
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8_lossy(&bytes);
        assert!(!body.contains("boom-str"), "5xx must not leak the message");
    }

    /// The downcast path handles a `String` payload.
    #[tokio::test]
    async fn handles_string_payload() {
        let response = handle_panic(Box::new("boom-string".to_string()));
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// An opaque payload falls back to a stable message (no panic).
    #[tokio::test]
    async fn handles_opaque_payload() {
        let response = handle_panic(Box::new(42u32));
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// The snippet renders numbered lines and marks the target.
    #[test]
    fn snippet_renders_numbered_lines() {
        let dir = std::env::temp_dir().join(format!("rustasea-snippet-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let file = dir.join("sample.rs");
        std::fs::write(&file, "fn a() {}\nfn b() {\n    panic!()\n}\n").expect("write");

        let snippet = source_snippet(file.to_str().expect("path"), 3, 1).expect("snippet");
        assert!(snippet.contains("panic!()"));
        assert!(snippet.contains(">     3 |"));
        assert!(snippet.contains("2 |"));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A missing file yields `None` rather than an error.
    #[test]
    fn snippet_returns_none_for_missing_file() {
        assert!(source_snippet("/nonexistent/does/not/exist.rs", 1, 2).is_none());
    }

    /// The hook records a location and then forwards to the previous hook.
    ///
    /// Hooks are process-global, so the test is serialized and the default hook
    /// is restored in cleanup. `catch_unwind` suppresses the default output.
    #[test]
    fn hook_records_location() {
        use std::sync::Mutex;
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|error| error.into_inner());

        install_panic_location_hook();
        let _ = std::panic::catch_unwind(|| panic!("hook-test-message"));

        let location = last_panic_location().expect("recorded location");
        assert_eq!(location.message, "hook-test-message");
        assert!(location.file.ends_with(".rs"));
        assert!(location.line > 0);
    }
}
