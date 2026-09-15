//! Unit tests for the `mcp:serve` backend (feature `mcp`).
//!
//! These drive [`RustaseaBackend`] in-process so tool/resource logic can be
//! asserted with injected state (routes), which a spawned server process would
//! not observe (the route registry is process-local).

use super::*;
use rustasea_router::RouteEntry;
use std::sync::Mutex;

/// Serializes tests sharing the process-wide route registry.
static LOCK: Mutex<()> = Mutex::new(());

/// Build a minimal route entry.
fn entry(method: &str, path: &str) -> RouteEntry {
    RouteEntry {
        method: method.to_string(),
        path: path.to_string(),
        name: None,
        middleware: Vec::new(),
        authorizations: Vec::new(),
        domain: None,
        binding_fields: Vec::new(),
        controller: None,
        handler: None,
    }
}

/// A backend with no project root (filesystem resources disabled).
fn headless() -> RustaseaBackend {
    RustaseaBackend::new(None)
}

/// The command advertises the `mcp:serve` signature and a help line.
#[test]
fn mcp_serve_command_metadata() {
    assert_eq!(McpServe.signature(), "mcp:serve");
    assert!(McpServe.help().is_some());
}

/// The four tools are advertised with input schemas.
#[test]
fn backend_advertises_four_tools() {
    let tools = headless().tools();
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["route:list", "model:show", "make:plan", "migrate:status"]
    );
    assert!(tools.iter().all(|tool| tool.input_schema.is_object()));
}

/// `route:list` returns the injected live route table.
#[test]
fn route_list_tool_returns_injected_routes() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    routes::set_routes(vec![entry("GET", "/dashboard")]);
    let value = headless()
        .call_tool("route:list", json!({}))
        .expect("route:list succeeds");
    routes::clear_route_source();
    assert_eq!(value[0]["path"], "/dashboard");
    assert_eq!(value[0]["method"], "GET");
}

/// `model:show` derives the file path and table name.
#[test]
fn model_show_derives_path_and_table() {
    let value = headless()
        .call_tool("model:show", json!({ "name": "UserProfile" }))
        .expect("model:show succeeds");
    assert_eq!(value["file"], "app/models/user_profile.rs");
    assert_eq!(value["table"], "user_profiles");
}

/// `model:show` rejects a non-identifier name.
#[test]
fn model_show_rejects_invalid_name() {
    let error = headless()
        .call_tool("model:show", json!({ "name": "bad name" }))
        .unwrap_err();
    assert!(error.contains("valid"), "unexpected error: {error}");
}

/// `make:plan` plans a controller path without writing anything.
#[test]
fn make_plan_returns_target_path() {
    let value = headless()
        .call_tool(
            "make:plan",
            json!({ "kind": "controller", "name": "UserController" }),
        )
        .expect("make:plan succeeds");
    assert_eq!(value["path"], "app/http/controllers/user_controller.rs");
    assert_eq!(value["writes"], false);
    assert_eq!(value["template"], "make:controller");
}

/// `make:plan` rejects an unknown generator kind.
#[test]
fn make_plan_rejects_unknown_kind() {
    let error = headless()
        .call_tool("make:plan", json!({ "kind": "bogus", "name": "Thing" }))
        .unwrap_err();
    assert!(error.contains("unknown generator kind"), "error: {error}");
}

/// `migrate:status` honestly reports the empty registry as unavailable.
#[test]
fn migrate_status_reports_unavailable_without_app() {
    let value = headless()
        .call_tool("migrate:status", json!({}))
        .expect("migrate:status succeeds");
    // The test binary registers no migrations, so the status is unavailable.
    assert_eq!(value["status"], "unavailable");
    assert!(value["hint"].as_str().is_some());
}

/// A denied config traversal URI returns a typed error.
#[test]
fn denied_config_traversal_is_error() {
    let error = headless()
        .read_resource("rustasea://config/../../.env")
        .unwrap_err();
    assert!(error.contains("denied"), "error: {error}");
}

/// An authority outside the allow-list is denied.
#[test]
fn denied_authority_is_error() {
    let error = headless()
        .read_resource("rustasea://secrets/token")
        .unwrap_err();
    assert!(error.contains("allow-list"), "error: {error}");
}

/// The redactor replaces sensitive values and keeps structure.
#[test]
fn scrub_redacts_sensitive_keys() {
    let mut value = json!({
        "app": { "name": "RustaSea", "app_key": "base64-secret" },
        "database": { "password": "hunter2", "token": "abc" }
    });
    scrub_value(&mut value);
    assert_eq!(value["app"]["name"], "RustaSea");
    assert_eq!(value["app"]["app_key"], REDACTED);
    assert_eq!(value["database"]["password"], REDACTED);
    assert_eq!(value["database"]["token"], REDACTED);
}

/// The path guard rejects traversal and absolute names.
#[test]
fn safe_relative_guard() {
    assert!(is_safe_relative("milestones.md"));
    assert!(is_safe_relative("adr/ADR-0001-foo.md"));
    assert!(!is_safe_relative("../.env"));
    assert!(!is_safe_relative("/etc/passwd"));
    assert!(!is_safe_relative("a/../../b"));
    assert!(!is_safe_relative(""));
}

/// `rustasea://routes` and `rustasea://commands` always resolve.
#[test]
fn static_resources_are_readable() {
    let backend = headless();
    assert!(backend.read_resource("rustasea://routes").is_ok());
    assert!(backend.read_resource("rustasea://commands").is_ok());
}
