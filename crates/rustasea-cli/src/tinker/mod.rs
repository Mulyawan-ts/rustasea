//! Interactive `tinker` REPL — session core and application source registry.
//!
//! `cargo artisan tinker` drops the operator into a read-eval-print loop for
//! exploring a booted application: inspecting configuration, resolving
//! container entries, and listing registered routes and commands. The
//! evaluation core ([`TinkerSession`]) is deliberately separated from the
//! terminal shell ([`crate::commands::tinker`]) so it can be unit-tested
//! without a TTY: [`TinkerSession::eval`] maps one input line onto a
//! [`TinkerOutcome`] and never panics.
//!
//! # Application source registry
//!
//! Like the route-table registry (see [`crate::routes`]), `rustasea-cli` is
//! framework-generic and must not depend on an application crate. The
//! application therefore implements [`TinkerSource`] over its own container and
//! config loader and publishes it here during boot; the REPL reads it back.
//! The slot is a `OnceLock<RwLock<Option<Arc<dyn TinkerSource>>>>` — a
//! write-once global whose value may be replaced (the boot is the only
//! production writer) and cleared by tests for isolation.
//!
//! Without a published source the REPL still works for the framework-generic
//! surfaces (`routes`, `commands`, `help`); config/container inspection then
//! prints a friendly hint instead of failing.
//!
//! # Database verbs
//!
//! The `db.query` and `model` verbs live in [`verbs`]; they open a short-lived
//! pool from the resolved database URL rather than reading the application
//! source, so a read-only query and a table count work without a booted
//! application. They are the only asynchronous commands: [`TinkerSession::eval`]
//! is `async` and awaits them while the introspection verbs stay synchronous.

use std::fmt::Write as _;
use std::sync::{Arc, OnceLock, RwLock};

mod verbs;

/// Application introspection surface the REPL reads.
///
/// Implemented by the application crate over its booted container + config
/// loader and published through [`set_tinker_source`]. Every method is
/// best-effort and returns `None`/empty rather than erroring, so a partially
/// wired application still yields a usable REPL.
pub trait TinkerSource: Send + Sync {
    /// Resolve a configuration key, rendered as a JSON string.
    ///
    /// Returns `None` when the key is absent or the value cannot be rendered.
    fn config(&self, key: &str) -> Option<String>;

    /// List the keys bound in the application container.
    fn container_keys(&self) -> Vec<String>;

    /// Summarise one container entry (its resolved value or binding kind).
    ///
    /// Returns `None` when the key is not bound.
    fn container_entry(&self, key: &str) -> Option<String>;

    /// The active application environment name, when known.
    fn environment(&self) -> Option<String>;
}

/// The process-wide application-source slot.
static TINKER_SOURCE: OnceLock<RwLock<Option<Arc<dyn TinkerSource>>>> = OnceLock::new();

/// Borrow the process-wide source slot, initializing it on first use.
fn slot() -> &'static RwLock<Option<Arc<dyn TinkerSource>>> {
    TINKER_SOURCE.get_or_init(|| RwLock::new(None))
}

/// Publish the application's tinker source for the REPL.
///
/// A later call replaces an earlier source (the boot is the only production
/// writer; tests replace/clear it for isolation).
pub fn set_tinker_source(source: impl TinkerSource + 'static) {
    if let Ok(mut guard) = slot().write() {
        *guard = Some(Arc::new(source));
    }
}

/// Return the published application source, if any.
///
/// `None` when no source has been published — for example the framework-generic
/// `cargo-artisan` binary, which never boots an application — or when the lock
/// is poisoned. Never panics.
pub fn tinker_source() -> Option<Arc<dyn TinkerSource>> {
    match slot().read() {
        Ok(guard) => guard.clone(),
        Err(_) => None,
    }
}

/// Remove the published source (test isolation).
pub fn clear_tinker_source() {
    if let Ok(mut guard) = slot().write() {
        *guard = None;
    }
}

/// The outcome of evaluating one REPL line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TinkerOutcome {
    /// A rendered result (or a friendly error) to print, then continue.
    Print(String),
    /// The user asked to leave the session (`exit` / `quit`).
    Exit,
    /// The line was blank; print nothing.
    Noop,
}

/// Message shown when a command needs an application source that is absent.
const NO_SOURCE: &str = "no application source published — start `tinker` from a booted \
                         application (e.g. `cargo run -p rustasea-app`) to inspect config \
                         and the container";

/// A TTY-free evaluation core for the `tinker` REPL.
///
/// Holds an optional [`TinkerSource`] and maps one input line onto a
/// [`TinkerOutcome`]. Errors are rendered as friendly [`TinkerOutcome::Print`]
/// messages so the surrounding loop never aborts on bad input.
pub struct TinkerSession {
    source: Option<Arc<dyn TinkerSource>>,
}

impl TinkerSession {
    /// Build a session over an explicit source (or `None` for framework-only).
    pub fn new(source: Option<Arc<dyn TinkerSource>>) -> Self {
        Self { source }
    }

    /// Build a session from the process-wide source registry.
    pub fn from_registry() -> Self {
        Self::new(tinker_source())
    }

    /// Evaluate one input line, returning the outcome to render.
    ///
    /// Recognised commands: `help`, `exit`/`quit`, `routes`, `commands`/`list`,
    /// `config <key>`, `container [key]`, `app`/`env`, `db.query <sql>`, and
    /// `model <table> [limit]`. Blank lines yield [`TinkerOutcome::Noop`]; an
    /// unrecognised command yields a friendly hint. The database verbs are the
    /// only asynchronous ones; every other command renders synchronously.
    pub async fn eval(&self, line: &str) -> TinkerOutcome {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return TinkerOutcome::Noop;
        }
        let mut parts = trimmed.split_whitespace();
        let Some(command) = parts.next() else {
            return TinkerOutcome::Noop;
        };
        let rest: Vec<&str> = parts.collect();
        match command {
            "exit" | "quit" => TinkerOutcome::Exit,
            "help" | "?" => TinkerOutcome::Print(help_text()),
            "routes" => TinkerOutcome::Print(self.routes()),
            "commands" | "list" => TinkerOutcome::Print(self.commands()),
            "config" => TinkerOutcome::Print(self.config(&rest)),
            "container" => TinkerOutcome::Print(self.container(&rest)),
            "app" | "env" => TinkerOutcome::Print(self.app()),
            "db.query" => TinkerOutcome::Print(verbs::db_query(&rest).await),
            "model" => TinkerOutcome::Print(verbs::model(&rest).await),
            other => TinkerOutcome::Print(format!(
                "unknown command `{other}`. Type `help` for available commands."
            )),
        }
    }

    /// Render the registered route table (framework-generic).
    fn routes(&self) -> String {
        let routes = crate::routes::routes();
        if routes.is_empty() {
            return "no routes registered".to_string();
        }
        let mut out = String::from("registered routes:\n");
        for route in routes {
            let name = route.name.as_deref().unwrap_or("-");
            let _ = writeln!(out, "  {:7} {:30} {}", route.method, route.path, name);
        }
        out.trim_end().to_string()
    }

    /// Render the registered command surface (framework-generic).
    fn commands(&self) -> String {
        let commands = crate::Artisan::list(true);
        if commands.is_empty() {
            return "no commands registered".to_string();
        }
        let mut out = String::from("registered commands:\n");
        for meta in commands {
            let _ = writeln!(out, "  {:22} {}", meta.name, meta.help);
        }
        out.trim_end().to_string()
    }

    /// Render a configuration value, or a friendly usage/absence message.
    fn config(&self, args: &[&str]) -> String {
        let Some(key) = args.first() else {
            return "usage: config <key>   (e.g. config app_name)".to_string();
        };
        let Some(source) = self.source.as_ref() else {
            return NO_SOURCE.to_string();
        };
        match source.config(key) {
            Some(value) => format!("{key} = {value}"),
            None => format!("no configuration value for `{key}`"),
        }
    }

    /// List container keys, or summarise one entry.
    fn container(&self, args: &[&str]) -> String {
        let Some(source) = self.source.as_ref() else {
            return NO_SOURCE.to_string();
        };
        match args.first() {
            None => {
                let keys = source.container_keys();
                if keys.is_empty() {
                    return "container is empty".to_string();
                }
                format!("container keys:\n  {}", keys.join("\n  "))
            }
            Some(key) => match source.container_entry(key) {
                Some(summary) => format!("{key} = {summary}"),
                None => format!("`{key}` is not bound in the container"),
            },
        }
    }

    /// Render the active application environment.
    fn app(&self) -> String {
        match self.source.as_ref().and_then(|source| source.environment()) {
            Some(environment) => format!("environment: {environment}"),
            None => "environment: unknown (no application source published)".to_string(),
        }
    }
}

/// The `help` text rendered by the REPL.
fn help_text() -> String {
    [
        "RustaSea tinker — interactive application inspector",
        "",
        "  help                  show this help",
        "  config <key>          inspect a configuration value (e.g. config app_name)",
        "  container             list container binding keys",
        "  container <key>       summarise one container entry",
        "  routes                list registered HTTP routes",
        "  commands              list registered console commands",
        "  app                   show the active environment",
        "  db.query <sql>        run a read-only query, print rows as JSON",
        "  model <table> [limit] count rows, or list up to `limit` rows as JSON",
        "  exit | quit           leave the session (Ctrl+D also exits)",
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustasea_router::RouteEntry;

    /// Serializes tests that share the process-wide registries.
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// A deterministic in-memory [`TinkerSource`] for tests.
    struct FakeSource;

    impl TinkerSource for FakeSource {
        fn config(&self, key: &str) -> Option<String> {
            (key == "app_name").then(|| "\"RustaSea\"".to_string())
        }

        fn container_keys(&self) -> Vec<String> {
            vec!["config.loader".to_string(), "app.environment".to_string()]
        }

        fn container_entry(&self, key: &str) -> Option<String> {
            (key == "app.environment").then(|| "local".to_string())
        }

        fn environment(&self) -> Option<String> {
            Some("local".to_string())
        }
    }

    /// Build a route entry at `path`.
    fn route(path: &str) -> RouteEntry {
        RouteEntry {
            method: "GET".into(),
            path: path.into(),
            name: Some("home".into()),
            middleware: Vec::new(),
            authorizations: Vec::new(),
            domain: None,
            binding_fields: Vec::new(),
            controller: None,
            handler: None,
        }
    }

    /// A session evaluates `config` and renders the resolved value.
    #[tokio::test]
    async fn session_evaluates_config_command() {
        let _guard = LOCK.lock().await;
        let session = TinkerSession::new(Some(Arc::new(FakeSource)));
        assert_eq!(
            session.eval("config app_name").await,
            TinkerOutcome::Print("app_name = \"RustaSea\"".to_string())
        );
    }

    /// A session lists container keys and summarises a bound entry.
    #[tokio::test]
    async fn session_inspects_container() {
        let _guard = LOCK.lock().await;
        let session = TinkerSession::new(Some(Arc::new(FakeSource)));
        match session.eval("container").await {
            TinkerOutcome::Print(text) => {
                assert!(text.contains("config.loader"), "output: {text}");
                assert!(text.contains("app.environment"), "output: {text}");
            }
            other => panic!("expected Print, got {other:?}"),
        }
        assert_eq!(
            session.eval("container app.environment").await,
            TinkerOutcome::Print("app.environment = local".to_string())
        );
    }

    /// A session renders the registered route table.
    #[tokio::test]
    async fn session_lists_routes() {
        let _guard = LOCK.lock().await;
        crate::routes::set_routes(vec![route("/dashboard")]);
        let session = TinkerSession::from_registry();

        let outcome = session.eval("routes").await;
        crate::routes::clear_route_source();

        match outcome {
            TinkerOutcome::Print(text) => assert!(text.contains("/dashboard"), "output: {text}"),
            other => panic!("expected Print, got {other:?}"),
        }
    }

    /// An unrecognised command prints a friendly hint and the session continues.
    #[tokio::test]
    async fn syntax_error_is_friendly_and_non_fatal() {
        let _guard = LOCK.lock().await;
        let session = TinkerSession::new(Some(Arc::new(FakeSource)));

        match session.eval("frobnicate").await {
            TinkerOutcome::Print(text) => {
                assert!(text.contains("unknown command"), "output: {text}");
                assert!(text.contains("help"), "output: {text}");
            }
            other => panic!("expected Print, got {other:?}"),
        }
        // The session keeps working after the error (REPL continues).
        assert_eq!(
            session.eval("config app_name").await,
            TinkerOutcome::Print("app_name = \"RustaSea\"".to_string())
        );
    }

    /// `config` without a key and an unknown key both yield friendly messages.
    #[tokio::test]
    async fn config_edge_cases_are_friendly() {
        let _guard = LOCK.lock().await;
        let session = TinkerSession::new(Some(Arc::new(FakeSource)));
        match session.eval("config").await {
            TinkerOutcome::Print(text) => assert!(text.contains("usage"), "output: {text}"),
            other => panic!("expected Print, got {other:?}"),
        }
        match session.eval("config missing_key").await {
            TinkerOutcome::Print(text) => {
                assert!(text.contains("no configuration value"), "output: {text}")
            }
            other => panic!("expected Print, got {other:?}"),
        }
    }

    /// Exit, blank lines, and help map onto their outcomes.
    #[tokio::test]
    async fn control_commands_map_to_outcomes() {
        let _guard = LOCK.lock().await;
        let session = TinkerSession::new(None);
        assert_eq!(session.eval("exit").await, TinkerOutcome::Exit);
        assert_eq!(session.eval("quit").await, TinkerOutcome::Exit);
        assert_eq!(session.eval("   ").await, TinkerOutcome::Noop);
        match session.eval("help").await {
            TinkerOutcome::Print(text) => assert!(text.contains("config <key>"), "output: {text}"),
            other => panic!("expected Print, got {other:?}"),
        }
    }

    /// Without a published source, config/container report the friendly hint.
    #[tokio::test]
    async fn missing_source_is_friendly() {
        let _guard = LOCK.lock().await;
        let session = TinkerSession::new(None);
        match session.eval("config app_name").await {
            TinkerOutcome::Print(text) => assert!(text.contains("no application source")),
            other => panic!("expected Print, got {other:?}"),
        }
        match session.eval("app").await {
            TinkerOutcome::Print(text) => assert!(text.contains("unknown")),
            other => panic!("expected Print, got {other:?}"),
        }
    }

    /// The registry publishes and clears the application source.
    #[tokio::test]
    async fn registry_publishes_and_clears_source() {
        let _guard = LOCK.lock().await;
        clear_tinker_source();
        assert!(tinker_source().is_none());

        set_tinker_source(FakeSource);
        let source = tinker_source().expect("source published");
        assert_eq!(source.environment().as_deref(), Some("local"));

        clear_tinker_source();
        assert!(tinker_source().is_none());
    }
}
