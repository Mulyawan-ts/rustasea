//! `tinker` REPL evaluation tests — the TTY-free session core.
//!
//! Interactive terminal tests are impractical in CI, so the REPL is split into
//! a testable evaluation core ([`rustasea_cli::TinkerSession`]) and a thin
//! rustyline shell. These tests drive the core directly: a command evaluates
//! and renders its result (positive), and a syntax error prints a friendly
//! message while the session keeps working (negative).
//!
//! The database verbs (`db.query`, `model`) resolve the database URL from the
//! process environment and open a short-lived pool per evaluation, so the tests
//! pin `DATABASE_URL` to a shared-cache in-memory SQLite database and hold the
//! seeding pool open for the duration of the run (an in-memory database lives
//! only while a connection is open).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rustasea_cli::{TinkerOutcome, TinkerSession, TinkerSource};
use rustasea_orm::{DbPool, Value};

/// Serializes tests sharing the process-wide registries and environment.
static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Isolate the process working directory in a fresh empty temp directory.
///
/// [`rustasea_cli`]'s `database_url()` falls back to
/// `ConfigLoader::load_from(&["config/database", "config/app"])`, and the
/// `config` crate resolves those relative paths against `env::current_dir()`
/// (it never searches upward). Cargo runs integration tests with the crate root
/// as cwd, where no `config/` exists — but running the test binary directly from
/// the workspace root exposes the real `config/database.toml`, so a verb that
/// should report `db unavailable` instead silently connects. Entering an empty
/// temp directory removes that ambient fallback for the duration of the test.
///
/// The original directory is restored and the temp directory removed when the
/// guard drops, even on panic, so sibling tests never observe the temporary cwd.
struct IsolatedCwd {
    original: PathBuf,
    temp: PathBuf,
}

impl IsolatedCwd {
    /// Enter a fresh empty temp directory and return the guard.
    fn enter() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp = std::env::temp_dir().join(format!(
            "rustasea-tinker-cwd-{}-{}",
            std::process::id(),
            unique
        ));
        std::fs::create_dir_all(&temp).expect("create temp cwd");
        let original = std::env::current_dir().expect("current dir");
        std::env::set_current_dir(&temp).expect("enter temp cwd");
        Self { original, temp }
    }
}

impl Drop for IsolatedCwd {
    /// Restore the original working directory and remove the temp directory.
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
        let _ = std::fs::remove_dir_all(&self.temp);
    }
}

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

/// Evaluate `line` on `session` and return the printed text.
async fn eval(session: &TinkerSession, line: &str) -> String {
    printed(session.eval(line).await)
}

/// Pin `DATABASE_URL` to a fresh shared-cache in-memory database and seed it.
///
/// Returns the open seeding pool, which the caller must keep alive: an
/// in-memory SQLite database is dropped once its last connection closes, so the
/// pool must outlive every subsequent `eval`. The caller restores the
/// environment after the test.
async fn seeded_memory_pool(tag: &str) -> DbPool {
    let url = format!("sqlite:file:rustasea_tinker_{tag}?mode=memory&cache=shared");
    std::env::remove_var("DATABASE__URL");
    std::env::set_var("DATABASE_URL", &url);

    let pool = DbPool::connect(&url).await.expect("seed pool connects");
    pool.execute_raw(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL)",
        &[],
    )
    .await
    .expect("create users");
    for id in 1..=3 {
        pool.execute_raw(
            "INSERT INTO users (id, email) VALUES ($1, $2)",
            &[Value::Int(id), Value::Text(format!("u{id}@example.test"))],
        )
        .await
        .expect("seed user");
    }
    pool
}

/// Remove the database environment variables set by a test.
fn clear_database_env() {
    std::env::remove_var("DATABASE_URL");
    std::env::remove_var("DATABASE__URL");
}

/// Restore `key` to `previous`, removing it when it was previously unset.
fn restore_env(key: &str, previous: Option<String>) {
    match previous {
        Some(value) => std::env::set_var(key, value),
        None => std::env::remove_var(key),
    }
}

/// Positive: a session evaluates a command and prints its result.
#[tokio::test]
async fn session_evaluates_command_and_prints_result() {
    let _guard = LOCK.lock().await;
    let session = session();

    assert_eq!(
        eval(&session, "config app_name").await,
        "app_name = \"RustaSea\""
    );
    assert_eq!(
        eval(&session, "container app.environment").await,
        "app.environment = local"
    );
    assert!(eval(&session, "container").await.contains("config.loader"));
    assert!(eval(&session, "app").await.contains("local"));
}

/// Positive: registered routes are rendered by the framework-generic surface.
#[tokio::test]
async fn session_renders_registered_routes() {
    let _guard = LOCK.lock().await;
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

    let outcome = TinkerSession::from_registry().eval("routes").await;
    rustasea_cli::routes::clear_route_source();

    assert!(printed(outcome).contains("/dashboard"));
}

/// Negative: a syntax error prints a friendly message and the REPL continues.
#[tokio::test]
async fn syntax_error_is_friendly_and_repl_continues() {
    let _guard = LOCK.lock().await;
    let session = session();

    let error = eval(&session, "this is not a command").await;
    assert!(error.contains("unknown command"), "output: {error}");
    assert!(error.contains("help"), "output: {error}");

    // The session still evaluates the next line — the REPL did not crash.
    assert_eq!(
        eval(&session, "config app_name").await,
        "app_name = \"RustaSea\""
    );
}

/// Negative: `config` without a key and an unknown key both stay friendly.
#[tokio::test]
async fn config_misuse_is_friendly() {
    let _guard = LOCK.lock().await;
    let session = session();

    assert!(eval(&session, "config").await.contains("usage"));
    assert!(eval(&session, "config nope")
        .await
        .contains("no configuration value"));
}

/// Control: `exit`/`quit` map to Exit; blank lines are no-ops.
#[tokio::test]
async fn exit_and_blank_lines_map_to_control_outcomes() {
    let _guard = LOCK.lock().await;
    let session = session();

    assert_eq!(session.eval("exit").await, TinkerOutcome::Exit);
    assert_eq!(session.eval("quit").await, TinkerOutcome::Exit);
    assert_eq!(session.eval("   ").await, TinkerOutcome::Noop);
}

/// Positive: `db.query` runs a read-only SELECT and renders pretty JSON.
#[tokio::test]
async fn db_query_renders_json() {
    let _guard = LOCK.lock().await;
    std::env::remove_var("DATABASE__URL");
    std::env::set_var("DATABASE_URL", "sqlite::memory:");
    let session = session();

    let output = eval(&session, "db.query SELECT 42 AS answer").await;
    clear_database_env();

    assert!(output.contains("\"answer\": 42"), "output: {output}");
    assert!(output.contains('\n'), "expected pretty JSON: {output}");
}

/// Positive: `model <table>` counts rows and `model <table> <n>` lists them.
#[tokio::test]
async fn model_counts_and_lists_rows() {
    let _guard = LOCK.lock().await;
    let pool = seeded_memory_pool("model").await;
    let session = session();

    let count = eval(&session, "model users").await;
    assert_eq!(count, "users: 3 rows");

    let listed = eval(&session, "model users 2").await;
    clear_database_env();
    pool.close().await;

    assert!(listed.contains("\"id\": 1"), "output: {listed}");
    assert!(listed.contains("\"id\": 2"), "output: {listed}");
    assert!(
        !listed.contains("\"id\": 3"),
        "limit must exclude row 3: {listed}"
    );
}

/// Negative: a SQL typo prints a friendly `db error:` and the REPL continues.
#[tokio::test]
async fn db_query_sql_error_is_friendly_and_repl_survives() {
    let _guard = LOCK.lock().await;
    std::env::remove_var("DATABASE__URL");
    std::env::set_var("DATABASE_URL", "sqlite::memory:");
    let session = session();

    let error = eval(&session, "db.query SELECT * FRM users").await;
    assert!(error.contains("db error:"), "output: {error}");

    // The session keeps working after the SQL error — the REPL did not crash.
    let ok = eval(&session, "db.query SELECT 1 AS ok").await;
    clear_database_env();
    assert!(ok.contains("\"ok\": 1"), "output: {ok}");
}

/// Negative: an injected table name and a malformed identifier are refused
/// before any connection is opened.
#[tokio::test]
async fn model_identifier_validation_rejects_injection() {
    let _guard = LOCK.lock().await;
    // No database is configured: the identifier guard must fire first, so the
    // output is the validation message rather than `db unavailable`.
    clear_database_env();
    let session = session();

    let injected = eval(&session, "model users; DROP TABLE users").await;
    assert!(
        injected.contains("invalid table name"),
        "output: {injected}"
    );
    assert!(!injected.contains("db unavailable"), "output: {injected}");

    let malformed = eval(&session, "model 1bad").await;
    assert!(
        malformed.contains("invalid table name"),
        "output: {malformed}"
    );
}

/// Negative: without a configured database the verbs report a friendly hint.
#[tokio::test]
async fn db_verbs_without_database_are_friendly() {
    let _guard = LOCK.lock().await;
    // Remember any ambient values so the process environment is left untouched.
    let previous_plain = std::env::var("DATABASE_URL").ok();
    let previous_nested = std::env::var("DATABASE__URL").ok();
    // Enter an empty temp cwd so the `config/database.toml` fallback cannot
    // resolve, then clear the environment. The cwd guard restores on drop, and
    // both are undone before asserting on the captured output.
    let cwd = IsolatedCwd::enter();
    clear_database_env();
    let session = session();

    let query = eval(&session, "db.query SELECT 1").await;
    let model = eval(&session, "model users").await;

    drop(cwd);
    restore_env("DATABASE_URL", previous_plain);
    restore_env("DATABASE__URL", previous_nested);

    assert!(query.contains("db unavailable"), "output: {query}");
    assert!(model.contains("db unavailable"), "output: {model}");
}

/// Positive: an empty result renders the zero-row marker.
#[tokio::test]
async fn db_query_empty_result_renders_zero_rows() {
    let _guard = LOCK.lock().await;
    std::env::remove_var("DATABASE__URL");
    std::env::set_var("DATABASE_URL", "sqlite::memory:");
    let session = session();

    let output = eval(&session, "db.query SELECT 1 WHERE 1 = 0").await;
    clear_database_env();

    assert_eq!(output, "(0 rows)");
}
