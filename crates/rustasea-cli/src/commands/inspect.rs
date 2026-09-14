//! Framework inspection commands — route:list and show:model.
//!
//! `route:list` emits the observable route table (FS-M1-03) and `show:model`
//! the model inspector surface (FR-109). Both honour `--json`.

use async_trait::async_trait;
use rustasea_router::RouteEntry;

use crate::artisan::{Command, Io};
use crate::error::{CliError, CliResult};
use crate::generator::Generator;
use crate::output;
use crate::routes;

/// Placeholder rendered for an absent optional route field.
const ABSENT: &str = "-";

/// `route:list` — enumerate registered HTTP routes.
pub struct RouteList;

#[async_trait]
impl Command for RouteList {
    /// Command signature.
    fn signature(&self) -> &'static str {
        "route:list"
    }

    /// Usage line rendered by `list`.
    fn usage(&self) -> Option<&'static str> {
        Some("route:list [--json]")
    }

    /// One-line help rendered by `list`.
    fn help(&self) -> Option<&'static str> {
        Some("List all registered routes")
    }

    /// Execute: render the live application route table.
    ///
    /// Reads the process-wide registry published by the application at boot
    /// (see [`crate::routes`]); an unset registry yields a header-only table or
    /// an empty JSON array, never an error.
    async fn run(&self, args: Vec<String>, io: &mut Io) -> CliResult<()> {
        let routes = routes::routes();
        if args.iter().any(|a| a == "--json") {
            io.line(serde_json::to_string(&routes)?);
            return Ok(());
        }
        let mut rows = vec![vec![
            "Method".to_string(),
            "URI".to_string(),
            "Name".to_string(),
            "Action".to_string(),
            "Middleware".to_string(),
        ]];
        for route in &routes {
            rows.push(vec![
                route.method.clone(),
                route.path.clone(),
                route.name.clone().unwrap_or_else(|| ABSENT.to_string()),
                action_label(route),
                middleware_label(route),
            ]);
        }
        io.line(output::table(rows).trim_end());
        Ok(())
    }
}

/// Render a route's dispatch target: bound handler, else controller ref.
fn action_label(route: &RouteEntry) -> String {
    if let Some(handler) = route.handler.as_deref() {
        return handler.to_string();
    }
    match &route.controller {
        Some(controller) => format!("{}@{}", controller.name, controller.action),
        None => ABSENT.to_string(),
    }
}

/// Render a route's middleware stack as a comma-separated list.
fn middleware_label(route: &RouteEntry) -> String {
    if route.middleware.is_empty() {
        return ABSENT.to_string();
    }
    route.middleware.join(", ")
}

/// `show:model` — inspect a model's attributes/relations/casts (FR-109).
pub struct ShowModel;

#[async_trait]
impl Command for ShowModel {
    /// Command signature.
    fn signature(&self) -> &'static str {
        "show:model"
    }

    /// Usage line rendered by `list`.
    fn usage(&self) -> Option<&'static str> {
        Some("show:model {name}")
    }

    /// One-line help rendered by `list`.
    fn help(&self) -> Option<&'static str> {
        Some("Show a model's attributes and relations")
    }

    /// Execute: print the model path + placeholder inspector output.
    async fn run(&self, args: Vec<String>, io: &mut Io) -> CliResult<()> {
        let name = args
            .first()
            .filter(|n| !n.starts_with('-'))
            .cloned()
            .unwrap_or_default();
        if name.is_empty() {
            return Err(CliError::InvalidArguments {
                command: "show:model".into(),
                detail: "expected a model name, e.g. show:model Post".into(),
            });
        }
        let snake = Generator::snake(&name);
        io.line(format!("model: {name}"));
        io.line(format!("file:  app/models/{snake}.rs"));
        io.line("table: <derived from Model::table_name()>");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that share the process-wide route registry.
    static ROUTE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Build a route entry carrying one middleware id.
    fn entry(method: &str, path: &str, name: Option<&str>) -> RouteEntry {
        RouteEntry {
            method: method.to_string(),
            path: path.to_string(),
            name: name.map(str::to_string),
            middleware: vec!["auth".to_string()],
            authorizations: Vec::new(),
            domain: None,
            binding_fields: Vec::new(),
            controller: None,
            handler: None,
        }
    }

    /// A published route table renders every route with all five columns.
    #[tokio::test]
    async fn route_list_renders_registered_routes() {
        let _guard = ROUTE_LOCK.lock().await;
        crate::routes::set_routes(vec![
            entry("GET", "/dashboard", Some("dashboard")),
            entry("GET", "/settings/profile", Some("settings.profile.edit")),
        ]);

        let mut io = Io::default();
        RouteList
            .run(vec![], &mut io)
            .await
            .expect("route:list runs");
        crate::routes::clear_route_source();

        assert!(io.stdout.contains("Method"), "stdout: {}", io.stdout);
        assert!(io.stdout.contains("URI"), "stdout: {}", io.stdout);
        assert!(io.stdout.contains("/dashboard"), "stdout: {}", io.stdout);
        assert!(
            io.stdout.contains("settings.profile.edit"),
            "stdout: {}",
            io.stdout
        );
        assert!(io.stdout.contains("auth"), "stdout: {}", io.stdout);
    }

    /// `--json` emits the structured route array.
    #[tokio::test]
    async fn route_list_json_emits_structured_array() {
        let _guard = ROUTE_LOCK.lock().await;
        crate::routes::set_routes(vec![entry("POST", "/login", Some("login"))]);

        let mut io = Io::default();
        RouteList
            .run(vec!["--json".to_string()], &mut io)
            .await
            .expect("route:list runs");
        crate::routes::clear_route_source();

        let parsed: serde_json::Value = serde_json::from_str(io.stdout.trim()).expect("valid JSON");
        let array = parsed.as_array().expect("JSON array");
        assert_eq!(array.len(), 1);
        assert_eq!(array[0]["method"], "POST");
        assert_eq!(array[0]["path"], "/login");
        assert_eq!(array[0]["name"], "login");
        assert_eq!(array[0]["middleware"][0], "auth");
    }

    /// An empty registry yields a header-only table and `[]` JSON.
    #[tokio::test]
    async fn route_list_empty_registry_is_clean() {
        let _guard = ROUTE_LOCK.lock().await;
        crate::routes::clear_route_source();

        let mut io = Io::default();
        RouteList
            .run(vec![], &mut io)
            .await
            .expect("route:list runs");
        assert!(io.stdout.contains("Method"), "stdout: {}", io.stdout);
        assert!(
            !io.stdout.contains('/'),
            "no routes expected: {}",
            io.stdout
        );

        let mut json_io = Io::default();
        RouteList
            .run(vec!["--json".to_string()], &mut json_io)
            .await
            .expect("route:list runs");
        assert_eq!(json_io.stdout.trim(), "[]");
    }
}
