//! `mcp:serve` — expose project knowledge over the Model Context Protocol.
//!
//! Runs the `rustasea-ai` MCP server engine on the process's stdin/stdout so an
//! AI coding agent can discover and read the application's knowledge surface
//! (laravel/boost parity). The concrete backend lives here — the protocol engine
//! stays in `rustasea-ai` and never depends on the CLI.
//!
//! # Surface
//!
//! * **Tools** — `route:list`, `model:show`, `make:plan`, `migrate:status`.
//! * **Resources** — `rustasea://docs/{name}`, `rustasea://config/{name}`,
//!   `rustasea://routes`, `rustasea://commands`.
//!
//! # Safety
//!
//! Reads are confined to an explicit allow-list: `docs/**.md` and
//! `config/*.toml` only, resolved project-root-relative with every `..`,
//! absolute, and separator-traversal segment rejected. Config values are
//! redacted with the same key-fragment matcher the Sentry scrubber uses, so
//! secrets (`password`, `secret`, `token`, `api_key`, …) never leave the
//! process. Nothing is ever written to disk.
//!
//! # Output
//!
//! Frames stream straight to the process streams and are flushed after every
//! response (see [`crate::commands::tinker`] for the same precedent), so the
//! [`Io`] buffer is intentionally left empty.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use rustasea_ai::mcp::{McpBackend, McpResource, McpResourceContent, McpTool, SERVER_NAME};

use crate::artisan::{Command, Io};
use crate::error::{CliError, CliResult};
use crate::generator::Generator;
use crate::generators::{self, Kind};
use crate::routes;

/// Key fragments that mark a config value as a credential (matched
/// case-insensitively). Mirrors the Sentry scrubber's list so redaction stays
/// consistent across the framework.
const SENSITIVE_KEY_FRAGMENTS: &[&str] = &[
    "password", "secret", "token", "api_key", "apikey", "app_key",
];

/// Placeholder written in place of a redacted config value.
const REDACTED: &str = "[Filtered]";

/// Longest config value (in bytes) exposed before truncation, so a huge inline
/// certificate or key blob cannot flood an agent's context window.
const MAX_FIELD_LEN: usize = 8 * 1024;

/// `mcp:serve` — serve the project knowledge over MCP stdio.
pub struct McpServe;

#[async_trait]
impl Command for McpServe {
    /// Command signature.
    fn signature(&self) -> &'static str {
        "mcp:serve"
    }

    /// Usage line rendered by `list`.
    fn usage(&self) -> Option<&'static str> {
        Some("mcp:serve")
    }

    /// One-line help rendered by `list`.
    fn help(&self) -> Option<&'static str> {
        Some("Serve the project knowledge (routes, docs, commands, config) over MCP stdio")
    }

    /// Execute: run the MCP server loop on process stdin/stdout.
    async fn run(&self, _args: Vec<String>, _io: &mut Io) -> CliResult<()> {
        let backend = RustaseaBackend::from_cwd();
        rustasea_ai::mcp::server::run_stdio(&backend)
            .await
            .map_err(|error| CliError::Domain(error.to_string()))
    }
}

/// Concrete [`McpBackend`] reading the project's routes, docs, and config.
pub struct RustaseaBackend {
    /// Project root (nearest ancestor with a `Cargo.toml`), if resolvable.
    root: Option<PathBuf>,
}

impl RustaseaBackend {
    /// Build a backend rooted at `root` (`None` disables filesystem resources).
    pub fn new(root: Option<PathBuf>) -> Self {
        Self { root }
    }

    /// Build a backend rooted at the current working directory's project root.
    ///
    /// Falls back to `None` when no `Cargo.toml` is found; tool calls still
    /// work (routes/commands are process-local), and filesystem resources
    /// return a typed error instead of reading outside the project.
    pub fn from_cwd() -> Self {
        let root = std::env::current_dir()
            .ok()
            .and_then(|cwd| crate::error::project_root(&cwd).ok());
        Self { root }
    }

    /// Resolve the project root, or a typed error when it is unknown.
    fn require_root(&self) -> Result<&Path, String> {
        self.root
            .as_deref()
            .ok_or_else(|| "not a RustaSea project: no Cargo.toml found above the cwd".to_string())
    }

    /// Read a `docs/*.md` or `docs/adr/*.md` file, or a typed error.
    ///
    /// The `{name}` may carry or omit the `.md` suffix (`milestones` and
    /// `milestones.md` both resolve to `docs/milestones.md`).
    fn read_doc(&self, name: &str) -> Result<McpResourceContent, String> {
        if !is_safe_relative(name) {
            return Err(format!("denied docs path `{name}`: not in the allow-list"));
        }
        let stem = name.strip_suffix(".md").unwrap_or(name);
        let components: Vec<&str> = stem.split('/').collect();
        let (dir, file) = match components.as_slice() {
            [file] => ("docs", *file),
            ["adr", file] => ("docs/adr", *file),
            _ => return Err(format!("denied docs path `{name}`: not in the allow-list")),
        };
        if file.is_empty() {
            return Err(format!("denied docs path `{name}`: not in the allow-list"));
        }
        let root = self.require_root()?;
        let path = root.join(dir).join(format!("{file}.md"));
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("failed to read docs resource `{name}`: {error}"))?;
        Ok(McpResourceContent {
            uri: format!("rustasea://docs/{stem}"),
            mime_type: "text/markdown".to_string(),
            text,
        })
    }

    /// Read and redact a `config/*.toml` file, or a typed error.
    fn read_config(&self, name: &str) -> Result<McpResourceContent, String> {
        if !is_safe_relative(name) || name.contains('/') || !name.ends_with(".toml") {
            return Err(format!(
                "denied config path `{name}`: only config/*.toml is exposed"
            ));
        }
        let root = self.require_root()?;
        let path = root.join("config").join(name);
        let raw = std::fs::read_to_string(&path)
            .map_err(|error| format!("failed to read config resource `{name}`: {error}"))?;
        let mut value: Value = toml::from_str(&raw)
            .map_err(|error| format!("failed to parse config resource `{name}`: {error}"))?;
        scrub_value(&mut value);
        let text = serde_json::to_string_pretty(&value)
            .map_err(|error| format!("failed to serialize config resource `{name}`: {error}"))?;
        Ok(McpResourceContent {
            uri: format!("rustasea://config/{name}"),
            mime_type: "application/json".to_string(),
            text,
        })
    }

    /// Read the live route table as JSON.
    fn read_routes(&self) -> Result<McpResourceContent, String> {
        let text = serde_json::to_string(&routes::routes())
            .map_err(|error| format!("failed to serialize routes: {error}"))?;
        Ok(McpResourceContent {
            uri: "rustasea://routes".to_string(),
            mime_type: "application/json".to_string(),
            text,
        })
    }

    /// Read the registered Artisan command list as JSON.
    fn read_commands(&self) -> Result<McpResourceContent, String> {
        let text = serde_json::to_string(&crate::Artisan::list(true))
            .map_err(|error| format!("failed to serialize commands: {error}"))?;
        Ok(McpResourceContent {
            uri: "rustasea://commands".to_string(),
            mime_type: "application/json".to_string(),
            text,
        })
    }
}

impl McpBackend for RustaseaBackend {
    /// Tools advertised by `tools/list`.
    fn tools(&self) -> Vec<McpTool> {
        tool(
            "route:list",
            "List all registered HTTP routes as JSON",
            json!({ "type": "object", "properties": {} }),
        )
        .into_iter()
        .chain(tool(
            "model:show",
            "Show a model's file path and derived table name",
            json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            }),
        ))
        .chain(tool(
            "make:plan",
            "Dry-run a make:* generator (target path + template); writes nothing",
            json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string" },
                    "name": { "type": "string" }
                },
                "required": ["kind", "name"]
            }),
        ))
        .chain(tool(
            "migrate:status",
            "Report the migrations registered in this binary",
            json!({ "type": "object", "properties": {} }),
        ))
        .collect()
    }

    /// Invoke a tool by name.
    fn call_tool(&self, name: &str, args: Value) -> Result<Value, String> {
        match name {
            "route:list" => serde_json::to_value(routes::routes())
                .map_err(|error| format!("failed to serialize routes: {error}")),
            "model:show" => self.model_show(&args),
            "make:plan" => self.make_plan(&args),
            "migrate:status" => Ok(migrate_status()),
            other => Err(format!("unknown tool `{other}`")),
        }
    }

    /// Resources advertised by `resources/list`.
    fn resources(&self) -> Vec<McpResource> {
        let mut resources = Vec::new();
        if let Some(root) = self.root.as_deref() {
            for file in list_files(&root.join("docs"), "md") {
                let stem = file.strip_suffix(".md").unwrap_or(&file).to_string();
                resources.push(McpResource {
                    uri: format!("rustasea://docs/{stem}"),
                    name: stem.clone(),
                    description: format!("Project documentation: {file}"),
                    mime_type: "text/markdown".to_string(),
                });
            }
            for file in list_files(&root.join("docs/adr"), "md") {
                let stem = file.strip_suffix(".md").unwrap_or(&file).to_string();
                resources.push(McpResource {
                    uri: format!("rustasea://docs/adr/{stem}"),
                    name: format!("adr/{stem}"),
                    description: format!("Architecture decision record: {file}"),
                    mime_type: "text/markdown".to_string(),
                });
            }
            for file in list_files(&root.join("config"), "toml") {
                resources.push(McpResource {
                    uri: format!("rustasea://config/{file}"),
                    name: format!("config/{file}"),
                    description: format!("Configuration (values redacted): {file}"),
                    mime_type: "application/json".to_string(),
                });
            }
        }
        resources.push(McpResource {
            uri: "rustasea://routes".to_string(),
            name: "routes".to_string(),
            description: "Live HTTP route table as JSON".to_string(),
            mime_type: "application/json".to_string(),
        });
        resources.push(McpResource {
            uri: "rustasea://commands".to_string(),
            name: "commands".to_string(),
            description: "Registered Artisan commands as JSON".to_string(),
            mime_type: "application/json".to_string(),
        });
        resources
    }

    /// Read an allow-listed resource by URI.
    fn read_resource(&self, uri: &str) -> Result<McpResourceContent, String> {
        if uri == "rustasea://routes" {
            return self.read_routes();
        }
        if uri == "rustasea://commands" {
            return self.read_commands();
        }
        let Some((authority, name)) = split_uri(uri) else {
            return Err(format!("denied resource `{uri}`: not in the allow-list"));
        };
        match authority {
            "docs" => self.read_doc(name),
            "config" => self.read_config(name),
            other => Err(format!(
                "denied resource `{uri}`: authority `{other}` is not in the allow-list"
            )),
        }
    }
}

impl RustaseaBackend {
    /// `model:show` — the model file path and derived table name.
    ///
    /// RustaSea has no runtime attribute/relation registry, so this reports
    /// only what is derivable from the name (honest by design).
    fn model_show(&self, args: &Value) -> Result<Value, String> {
        let name = required_string(args, "name")?;
        Generator::validate_name("model", &name).map_err(|error| error.to_string())?;
        Ok(json!({
            "name": name,
            "file": format!("app/models/{}.rs", Generator::snake(&name)),
            "table": Generator::table(&name),
            "note": "RustaSea has no attribute/relation registry; only the file path and derived table name are known statically."
        }))
    }

    /// `make:plan` — dry-run a generator without writing any file.
    fn make_plan(&self, args: &Value) -> Result<Value, String> {
        let kind_label = required_string(args, "kind")?;
        let name = required_string(args, "name")?;
        let kind = Kind::parse(&kind_label)
            .ok_or_else(|| format!("unknown generator kind `{kind_label}`"))?;
        if kind == Kind::Migration {
            if !generators::kinds::migration::validate_name(&name) {
                return Err(format!(
                    "`{name}` is not a valid migration name: expected snake_case"
                ));
            }
        } else {
            Generator::validate_name(&kind_label, &name).map_err(|error| error.to_string())?;
        }
        Ok(json!({
            "kind": kind_label,
            "name": name,
            "template": kind.command(),
            "path": generators::planned_path(kind, &name),
            "writes": false,
            "note": "Dry-run plan only; no files were written."
        }))
    }
}

/// Build one tool descriptor for the `rustasea` server.
fn tool(name: &str, description: &str, input_schema: Value) -> Option<McpTool> {
    Some(McpTool {
        server: SERVER_NAME.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
    })
}

/// Report the migrations registered in this binary.
///
/// The CLI has no database connection, so it cannot distinguish applied from
/// pending migrations. It honestly reports the registered set, or an
/// `unavailable` status when nothing was registered (a framework-generic
/// binary that never booted an application).
fn migrate_status() -> Value {
    let migrator = rustasea_orm::registered_migrator();
    if migrator.is_empty() {
        return json!({
            "status": "unavailable",
            "hint": "run from an application context"
        });
    }
    json!({
        "status": "registered",
        "migrations": migrator.names(),
        "note": "Applied/pending state requires a database connection, which the CLI does not open here."
    })
}

/// Extract a required non-empty string argument.
fn required_string(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("missing required string argument `{key}`"))
}

/// Split a `rustasea://<authority>/<name>` URI into its parts.
fn split_uri(uri: &str) -> Option<(&str, &str)> {
    let rest = uri.strip_prefix("rustasea://")?;
    rest.split_once('/')
}

/// Whether `name` is a safe relative path confined to the allow-list.
///
/// Rejects absolute paths, `..` traversal, backslashes, and empty/`.`/`..`
/// components, so a resource name can never escape its allow-listed directory.
fn is_safe_relative(name: &str) -> bool {
    !name.is_empty()
        && !name.contains("..")
        && !name.starts_with('/')
        && !name.contains('\\')
        && name
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

/// List the file names (not paths) in `dir` ending with `.ext`, sorted.
///
/// Missing directories yield an empty list rather than an error, so a project
/// without an `adr/` directory still lists its other resources.
fn list_files(dir: &Path, ext: &str) -> Vec<String> {
    let mut names: Vec<String> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.ends_with(&format!(".{ext}")))
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

/// Recursively redact sensitive keys and truncate oversized values.
fn scrub_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, entry) in map.iter_mut() {
                if is_sensitive_key(key) {
                    *entry = Value::from(REDACTED);
                } else {
                    scrub_value(entry);
                }
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                scrub_value(item);
            }
        }
        Value::String(text) if text.len() > MAX_FIELD_LEN => {
            text.truncate(MAX_FIELD_LEN);
            text.push_str("…[truncated]");
        }
        _ => {}
    }
}

/// Whether `key` looks like a credential (contains a sensitive fragment).
fn is_sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    SENSITIVE_KEY_FRAGMENTS
        .iter()
        .any(|fragment| key.contains(fragment))
}

#[cfg(test)]
mod tests;
