//! `tinker` REPL evaluation tests — the TTY-free session core.
//!
//! Interactive terminal tests are impractical in CI, so the REPL is split into
//! a testable evaluation core ([`rustasea_cli::TinkerSession`]) and a thin
//! rustyline shell. These tests drive the core directly: a command evaluates
//! and renders its result (positive), and a syntax error prints a friendly
//! message while the session keeps working (negative).

use std::sync::Arc;
use std::sync::Mutex;

use rustasea_cli::{TinkerOutcome, TinkerSession, TinkerSource};

/// Serializes tests sharing the process-wide registries.
static LOCK: Mutex<()> = Mutex::new(());

/// Deterministic in-memory application source for the tests.
struct FakeSource;

impl TinkerSource for FakeSource {
    /// Only `app_name` resolves, rendered as a JSON string.
    fn config(&self, key: &str) -> Option<String> {
        (key == "app_name").then(|| "\"RustaSea\"".to_string())
    }

    /// Two well-known bindings.
    fn container_keys(&self) -> Vec<String> {
        vec!["app.environment".to_string(), "config.loader".to_string()]
    }

    /// Only `app.environment` summarises.
    fn container_entry(&self, key: &str) -> Option<String> {
        (key == "app.environment").then(|| "local".to_string())
    }

    /// The active environment.
    fn environment(&self) -> Option<String> {
        Some("local".to_string())
    }
}

/// Build a session over the fake source.
fn session() -> TinkerSession {
    TinkerSession::new(Some(Arc::new(FakeSource)))
}

/// Extract the printed text from an outcome, panicking otherwise.
fn printed(outcome: TinkerOutcome) -> String {
    match outcome {
        TinkerOutcome::Print(text) => text,
        other => panic!("expected Print, got {other:?}"),
    }
}

/// Positive: a session evaluates a command and prints its result.
#[test]
fn session_evaluates_command_and_prints_result() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let session = session();

    assert_eq!(
        printed(session.eval("config app_name")),
        "app_name = \"RustaSea\""
    );
    assert_eq!(
        printed(session.eval("container app.environment")),
        "app.environment = local"
    );
    assert!(printed(session.eval("container")).contains("config.loader"));
    assert!(printed(session.eval("app")).contains("local"));
}

/// Positive: registered routes are rendered by the framework-generic surface.
#[test]
fn session_renders_registered_routes() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    rustasea_cli::routes::set_routes(vec![rustasea_router::RouteEntry {
        method: "GET".into(),
        path: "/dashboard".into(),
        name: Some("dashboard".into()),
        middleware: Vec::new(),
        authorizations: Vec::new(),
        domain: None,
        binding_fields: Vec::new(),
        controller: None,
        handler: None,
    }]);

    let outcome = TinkerSession::from_registry().eval("routes");
    rustasea_cli::routes::clear_route_source();

    assert!(printed(outcome).contains("/dashboard"));
}

/// Negative: a syntax error prints a friendly message and the REPL continues.
#[test]
fn syntax_error_is_friendly_and_repl_continues() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let session = session();

    let error = printed(session.eval("this is not a command"));
    assert!(error.contains("unknown command"), "output: {error}");
    assert!(error.contains("help"), "output: {error}");

    // The session still evaluates the next line — the REPL did not crash.
    assert_eq!(
        printed(session.eval("config app_name")),
        "app_name = \"RustaSea\""
    );
}

/// Negative: `config` without a key and an unknown key both stay friendly.
#[test]
fn config_misuse_is_friendly() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let session = session();

    assert!(printed(session.eval("config")).contains("usage"));
    assert!(printed(session.eval("config nope")).contains("no configuration value"));
}

/// Control: `exit`/`quit` map to Exit; blank lines are no-ops.
#[test]
fn exit_and_blank_lines_map_to_control_outcomes() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let session = session();

    assert_eq!(session.eval("exit"), TinkerOutcome::Exit);
    assert_eq!(session.eval("quit"), TinkerOutcome::Exit);
    assert_eq!(session.eval("   "), TinkerOutcome::Noop);
}
