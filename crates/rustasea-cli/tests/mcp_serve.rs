//! End-to-end round trip: an `McpClient` drives the real `cargo-artisan
//! mcp:serve` binary over stdio (feature `mcp`).
//!
//! Positive: the handshake completes, the four tools are discovered, a tool
//! returns structured JSON, the allow-listed resources are listed, and a docs
//! resource reads back as Markdown. Negative: a denied resource URI and an
//! unknown tool both surface typed errors rather than a panic.
//!
//! The child process inherits this test's working directory, and the binary
//! resolves its project root by walking up to the nearest `Cargo.toml`. Cargo
//! runs integration tests from the crate root (`crates/rustasea-cli`), which has
//! no `docs/`/`config/`; the test therefore re-roots the process at the
//! workspace root (two levels up) so the filesystem resources resolve. There is
//! a single test so this process-global `chdir` cannot race a sibling.
#![cfg(feature = "mcp")]

use std::path::Path;

use rustasea_ai::mcp::{McpClient, McpServerConfig};
use rustasea_ai::AiError;
use serde_json::json;

/// The compiled `cargo-artisan` binary under test (set by Cargo).
const BIN: &str = env!("CARGO_BIN_EXE_cargo-artisan");

/// The workspace root (two directories above this crate's manifest).
fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|crate_dir| crate_dir.parent())
        .expect("workspace root above the crate")
}

/// POSITIVE + NEGATIVE — full MCP round trip against the real binary.
#[tokio::test]
async fn mcp_serve_round_trip() {
    // Re-root so the spawned binary finds the workspace `docs/` and `config/`.
    std::env::set_current_dir(workspace_root()).expect("enter workspace root");

    let config = McpServerConfig::stdio_with_args("rustasea", BIN, ["mcp:serve"]);
    let mut client = McpClient::connect(&config)
        .await
        .expect("handshake completes");

    // POSITIVE — the four tools are advertised.
    let tools = client.list_tools().await.expect("list tools");
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["route:list", "model:show", "make:plan", "migrate:status"]
    );

    // POSITIVE — `route:list` returns a JSON array (empty without a booted app).
    let result = client
        .call_tool("route:list", json!({}))
        .await
        .expect("call route:list");
    let text = result["content"][0]["text"].as_str().expect("text content");
    let routes: serde_json::Value = serde_json::from_str(text).expect("routes are JSON");
    assert!(routes.is_array(), "route:list must return an array");

    // POSITIVE — `make:plan` reports a dry-run path and writes nothing.
    let plan = client
        .call_tool(
            "make:plan",
            json!({ "kind": "controller", "name": "UserController" }),
        )
        .await
        .expect("call make:plan");
    let plan_text = plan["content"][0]["text"].as_str().expect("text content");
    let plan: serde_json::Value = serde_json::from_str(plan_text).expect("plan is JSON");
    assert_eq!(plan["path"], "app/http/controllers/user_controller.rs");
    assert_eq!(plan["writes"], false);

    // POSITIVE — the allow-listed resources are listed.
    let resources = client.list_resources().await.expect("list resources");
    let uris: Vec<&str> = resources.iter().map(|r| r.uri.as_str()).collect();
    assert!(
        uris.contains(&"rustasea://docs/milestones"),
        "docs resource missing: {uris:?}"
    );
    assert!(
        uris.contains(&"rustasea://config/app.toml"),
        "config resource missing: {uris:?}"
    );
    assert!(uris.contains(&"rustasea://routes"));
    assert!(uris.contains(&"rustasea://commands"));

    // POSITIVE — a docs resource reads back as Markdown.
    let doc = client
        .read_resource("rustasea://docs/milestones")
        .await
        .expect("read docs/milestones");
    assert_eq!(doc.mime_type, "text/markdown");
    assert!(!doc.text.is_empty(), "docs body must not be empty");

    // POSITIVE — a config resource is readable and redacted (app_key present).
    let config_doc = client
        .read_resource("rustasea://config/app.toml")
        .await
        .expect("read config/app.toml");
    assert!(
        config_doc.text.contains("[Filtered]"),
        "app_key must be redacted"
    );

    // NEGATIVE — a denied resource URI surfaces a typed error.
    let denied = client
        .read_resource("rustasea://config/../../.env")
        .await
        .unwrap_err();
    assert!(
        matches!(denied, AiError::McpProtocol { .. }),
        "unexpected error: {denied}"
    );

    // NEGATIVE — an unknown tool surfaces a typed error (isError: true).
    let tool_error = client
        .call_tool("does:not:exist", json!({}))
        .await
        .unwrap_err();
    assert!(
        matches!(tool_error, AiError::McpProtocol { .. }),
        "unexpected error: {tool_error}"
    );
}
