//! Framework inspection commands — route:list and show:model.
//!
//! `route:list` emits the observable route table (FS-M1-03) and `show:model`
//! the model inspector surface (FR-109). Both honour `--json`.

use async_trait::async_trait;
use rustasea_router::RouteEntry;

use crate::artisan::{Command, Io};
use crate::error::{project_root, CliError, CliResult};
use crate::generator::Generator;
use crate::model_inspect::{inspect_model, ModelInspection};
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
            "Binding".to_string(),
        ]];
        for route in &routes {
            rows.push(vec![
                route.method.clone(),
                route.path.clone(),
                route.name.clone().unwrap_or_else(|| ABSENT.to_string()),
                action_label(route),
                middleware_label(route),
                binding_label(route),
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

/// Render a route's path-binding fields as a comma-separated list.
///
/// These are the `{var}` / `{var:field}` segments parsed from the route path
/// (Laravel feature #20 — `route:list` binding fields), so an operator can see
/// which parameters a route extracts without reading its definition.
fn binding_label(route: &RouteEntry) -> String {
    if route.binding_fields.is_empty() {
        return ABSENT.to_string();
    }
    route.binding_fields.join(", ")
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
        Some("show:model {name} [--json]")
    }

    /// One-line help rendered by `list`.
    fn help(&self) -> Option<&'static str> {
        Some("Show a model's attributes, casts and relations")
    }

    /// Execute: introspect `app/models/{snake}.rs` and print the surface.
    ///
    /// Resolves the project root (nearest `Cargo.toml`), parses the model file
    /// with [`crate::model_inspect`], and renders either the human summary or
    /// the `--json` contract shape.
    async fn run(&self, args: Vec<String>, io: &mut Io) -> CliResult<()> {
        let json = args.iter().any(|a| a == "--json");
        let name = args
            .iter()
            .find(|n| !n.starts_with('-'))
            .cloned()
            .unwrap_or_default();
        if name.is_empty() {
            return Err(CliError::InvalidArguments {
                command: "show:model".into(),
                detail: "expected a model name, e.g. show:model Post".into(),
            });
        }
        let cwd = std::env::current_dir().map_err(CliError::Io)?;
        let root = project_root(&cwd)?;
        render_model(&root, &name, json, io)
    }
}

/// Introspect `name` under `root` and render it (human or JSON).
///
/// Split from [`ShowModel::run`] so tests can drive a fixture project root
/// directly, without mutating the process-wide working directory.
fn render_model(root: &std::path::Path, name: &str, json: bool, io: &mut Io) -> CliResult<()> {
    let inspection =
        inspect_model(root, name).map_err(|error| CliError::Domain(error.to_string()))?;
    if json {
        io.line(serde_json::to_string(&inspection)?);
        return Ok(());
    }
    render_human(&inspection, name, io);
    Ok(())
}

/// Render the human-readable model summary (mirrors `route:list` styling).
fn render_human(inspection: &ModelInspection, name: &str, io: &mut Io) {
    io.line(format!("model: {}", inspection.model));
    io.line(format!("file:  app/models/{}.rs", Generator::snake(name)));
    io.line(format!("table: {}", inspection.table));
    io.line(format!("soft deletes: {}", yes_no(inspection.soft_delete)));
    io.line(format!("timestamps:   {}", yes_no(inspection.timestamps)));

    io.line("attributes:");
    for attribute in &inspection.attributes {
        let suffix = if attribute.nullable {
            " (nullable)"
        } else {
            ""
        };
        io.line(format!(
            "  - {}: {}{}",
            attribute.name, attribute.type_name, suffix
        ));
    }

    io.line("casts:");
    for (column, cast) in &inspection.casts {
        io.line(format!("  - {column}: {cast}"));
    }

    io.line("relations:");
    for relation in &inspection.relations {
        io.line(format!("  - {} {}", relation.kind, relation.name));
    }
}

/// Render a boolean flag as `yes`/`no`.
fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that share the process-wide route registry.
    static ROUTE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Build a route entry carrying one middleware id and any path bindings.
    fn entry(method: &str, path: &str, name: Option<&str>) -> RouteEntry {
        RouteEntry {
            method: method.to_string(),
            path: path.to_string(),
            name: name.map(str::to_string),
            middleware: vec!["auth".to_string()],
            authorizations: Vec::new(),
            domain: None,
            binding_fields: binding_fields(path),
            controller: None,
            handler: None,
        }
    }

    /// Extract `{var}` / `{var:field}` names from a path (test-local mirror).
    fn binding_fields(path: &str) -> Vec<String> {
        let mut fields = Vec::new();
        for segment in path.split('/') {
            let Some(inner) = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) else {
                continue;
            };
            let name = inner.split(':').next().unwrap_or(inner);
            if !name.is_empty() {
                fields.push(name.to_string());
            }
        }
        fields
    }

    /// A published route table renders every route with all six columns.
    #[tokio::test]
    async fn route_list_renders_registered_routes() {
        let _guard = ROUTE_LOCK.lock().await;
        crate::routes::set_routes(vec![
            entry("GET", "/dashboard", Some("dashboard")),
            entry("GET", "/settings/profile", Some("settings.profile.edit")),
            entry("GET", "/users/{user:slug}", Some("users.show")),
        ]);

        let mut io = Io::default();
        RouteList
            .run(vec![], &mut io)
            .await
            .expect("route:list runs");
        crate::routes::clear_route_source();

        assert!(io.stdout.contains("Method"), "stdout: {}", io.stdout);
        assert!(io.stdout.contains("URI"), "stdout: {}", io.stdout);
        assert!(io.stdout.contains("Binding"), "stdout: {}", io.stdout);
        assert!(io.stdout.contains("/dashboard"), "stdout: {}", io.stdout);
        assert!(
            io.stdout.contains("settings.profile.edit"),
            "stdout: {}",
            io.stdout
        );
        assert!(io.stdout.contains("auth"), "stdout: {}", io.stdout);
        assert!(io.stdout.contains("user"), "stdout: {}", io.stdout);
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

    /// `--json` includes the parsed binding fields for a parameterised route.
    #[tokio::test]
    async fn route_list_json_includes_binding_fields() {
        let _guard = ROUTE_LOCK.lock().await;
        crate::routes::set_routes(vec![entry("GET", "/users/{user:slug}", Some("users.show"))]);

        let mut io = Io::default();
        RouteList
            .run(vec!["--json".to_string()], &mut io)
            .await
            .expect("route:list runs");
        crate::routes::clear_route_source();

        let parsed: serde_json::Value = serde_json::from_str(io.stdout.trim()).expect("valid JSON");
        assert_eq!(parsed[0]["binding_fields"][0], "user");
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

    /// A model fixture source with attributes, a cast and a relation.
    const MODEL_SRC: &str = r#"
#[derive(Model)]
struct User {
    id: Uuid,
    name: String,
    #[model(cast = "json")]
    settings: Settings,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

impl Model for User {
    fn relations() -> Vec<Relation> {
        vec![Relation::has_many("posts", "posts", "User")]
    }
}
"#;

    /// Build a throwaway project root with `app/models/user.rs`.
    fn model_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let models = dir.path().join("app/models");
        std::fs::create_dir_all(&models).expect("mkdir");
        std::fs::write(models.join("user.rs"), MODEL_SRC).expect("write fixture");
        dir
    }

    /// The human summary lists the table, attributes, casts and relations.
    #[test]
    fn show_model_renders_human_summary() {
        let dir = model_fixture();
        let mut io = Io::default();
        render_model(dir.path(), "User", false, &mut io).expect("renders");

        assert!(io.stdout.contains("model: User"), "stdout: {}", io.stdout);
        assert!(io.stdout.contains("table: users"), "stdout: {}", io.stdout);
        assert!(
            io.stdout.contains("soft deletes: yes"),
            "stdout: {}",
            io.stdout
        );
        assert!(
            io.stdout.contains("timestamps:   yes"),
            "stdout: {}",
            io.stdout
        );
        assert!(io.stdout.contains("  - id: Uuid"), "stdout: {}", io.stdout);
        assert!(
            io.stdout
                .contains("  - deleted_at: Option<DateTime<Utc>> (nullable)"),
            "stdout: {}",
            io.stdout
        );
        assert!(
            io.stdout.contains("  - settings: json"),
            "stdout: {}",
            io.stdout
        );
        assert!(
            io.stdout.contains("  - HasMany posts"),
            "stdout: {}",
            io.stdout
        );
    }

    /// `--json` emits the contract shape (model/table/attributes/relations).
    #[test]
    fn show_model_json_emits_contract_shape() {
        let dir = model_fixture();
        let mut io = Io::default();
        render_model(dir.path(), "User", true, &mut io).expect("renders");

        let parsed: serde_json::Value = serde_json::from_str(io.stdout.trim()).expect("valid JSON");
        assert_eq!(parsed["model"], "User");
        assert_eq!(parsed["table"], "users");
        assert_eq!(parsed["soft_delete"], true);
        assert_eq!(parsed["attributes"][0]["name"], "id");
        assert_eq!(parsed["attributes"][0]["type"], "Uuid");
        assert_eq!(parsed["relations"][0]["kind"], "HasMany");
        assert_eq!(parsed["relations"][0]["name"], "posts");
        assert_eq!(parsed["casts"]["settings"], "json");
    }

    /// A missing model surfaces an error naming the searched path.
    #[test]
    fn show_model_reports_missing_path() {
        let dir = model_fixture();
        let mut io = Io::default();
        let error = render_model(dir.path(), "Ghost", false, &mut io).expect_err("missing");
        assert!(
            error.to_string().contains("app/models/ghost.rs"),
            "error: {error}"
        );
    }

    /// The hand-written `make:model` scaffold style is introspected too.
    ///
    /// Mirrors `crates/rustasea-cli/src/generators/kinds/model.rs`, whose
    /// output declares `impl Model` by hand with marker fields rather than
    /// `#[derive(Model)]`.
    const SCAFFOLD_SRC: &str = r#"
use rustasea::orm::{Model, SoftDeletes, Timestamps};

pub struct Post {
    pub id: uuid::Uuid,
    pub title: String,
    pub timestamps: Timestamps,
    pub soft_deletes: SoftDeletes,
}

impl Model for Post {
    fn type_name() -> &'static str { "Post" }
    fn table_name() -> String { "posts".to_string() }
    fn primary_key(&self) -> uuid::Uuid { self.id }
    fn assign_id(&mut self) -> uuid::Uuid { self.id }
}
"#;

    /// A hand-written scaffold model renders its table and tracked flags.
    #[test]
    fn show_model_handles_hand_written_scaffold() {
        let dir = tempfile::tempdir().expect("tempdir");
        let models = dir.path().join("app/models");
        std::fs::create_dir_all(&models).expect("mkdir");
        std::fs::write(models.join("post.rs"), SCAFFOLD_SRC).expect("write fixture");

        let mut io = Io::default();
        render_model(dir.path(), "Post", false, &mut io).expect("renders");

        assert!(io.stdout.contains("model: Post"), "stdout: {}", io.stdout);
        assert!(io.stdout.contains("table: posts"), "stdout: {}", io.stdout);
        assert!(
            io.stdout.contains("soft deletes: yes"),
            "stdout: {}",
            io.stdout
        );
        assert!(
            io.stdout.contains("timestamps:   yes"),
            "stdout: {}",
            io.stdout
        );
    }
}
