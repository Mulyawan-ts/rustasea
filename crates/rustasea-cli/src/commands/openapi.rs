//! `openapi:generate` — emit an OpenAPI 3.1 document from the live route table.
//!
//! The command reads the same process-wide route registry `route:list` renders
//! (see [`crate::routes`]) and feeds it to [`rustasea_openapi::generate`], so the
//! documented surface is exactly the table the application booted. An empty
//! registry still yields a valid document with an empty `paths` object.

use async_trait::async_trait;

use crate::artisan::{Command, Io};
use crate::error::{CliError, CliResult};
use crate::routes;

/// Default output path when `--output` is omitted.
const DEFAULT_OUTPUT: &str = "openapi.json";

/// `openapi:generate` — write the OpenAPI 3.1 specification to a file.
pub struct OpenApiGenerate;

#[async_trait]
impl Command for OpenApiGenerate {
    /// Command signature.
    fn signature(&self) -> &'static str {
        "openapi:generate"
    }

    /// Usage line rendered by `list`.
    fn usage(&self) -> Option<&'static str> {
        Some("openapi:generate [--output=openapi.json] [--pretty]")
    }

    /// One-line help rendered by `list`.
    fn help(&self) -> Option<&'static str> {
        Some("Generate the OpenAPI 3.1 specification")
    }

    /// Execute: generate the spec from the route registry and write it to disk.
    ///
    /// `--output=<path>` selects the destination (default `openapi.json`);
    /// `--pretty` writes indented JSON, otherwise the compact form. A route
    /// table that cannot be represented (an unknown method, a malformed path)
    /// surfaces as a typed [`CliError::Domain`] rather than a partial document.
    async fn run(&self, args: Vec<String>, io: &mut Io) -> CliResult<()> {
        let output = output_path(&args);
        let pretty = args.iter().any(|a| a == "--pretty");

        let entries = routes::routes();
        let spec = rustasea_openapi::generate(&entries)
            .map_err(|error| CliError::Domain(format!("{}: {error}", error.code())))?;

        let rendered = if pretty {
            serde_json::to_string_pretty(&spec)?
        } else {
            serde_json::to_string(&spec)?
        };
        std::fs::write(&output, rendered)?;
        io.line(format!("OpenAPI spec written to {output}"));
        Ok(())
    }
}

/// Resolve the `--output=<path>` value, defaulting to [`DEFAULT_OUTPUT`].
///
/// Accepts both `--output=path` and `--output path`; a bare `--output` with no
/// following value falls back to the default rather than erroring.
fn output_path(args: &[String]) -> String {
    for (index, arg) in args.iter().enumerate() {
        if let Some(value) = arg.strip_prefix("--output=") {
            if !value.is_empty() {
                return value.to_string();
            }
        } else if arg == "--output" {
            if let Some(value) = args.get(index + 1).filter(|v| !v.starts_with('-')) {
                return value.clone();
            }
        }
    }
    DEFAULT_OUTPUT.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rustasea_router::RouteEntry;

    /// Serializes tests that share the process-wide route registry.
    static ROUTE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Monotonic counter so parallel runs never collide on a temp path.
    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    /// Build a route entry at `path`.
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

    /// A unique temporary output path under the system temp directory.
    fn temp_output() -> std::path::PathBuf {
        let unique = SEQUENCE.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "rustasea-openapi-{}-{unique}.json",
            std::process::id()
        ))
    }

    /// `--output` selects the destination and the written file is a valid spec.
    #[tokio::test]
    async fn generates_spec_to_the_requested_path() {
        let _guard = ROUTE_LOCK.lock().await;
        crate::routes::set_routes(vec![entry("GET", "/users/{id}")]);

        let output = temp_output();
        let mut io = Io::default();
        OpenApiGenerate
            .run(
                vec![
                    format!("--output={}", output.display()),
                    "--pretty".to_string(),
                ],
                &mut io,
            )
            .await
            .expect("openapi:generate runs");
        crate::routes::clear_route_source();

        let written = std::fs::read_to_string(&output).expect("spec file written");
        let _ = std::fs::remove_file(&output);

        let parsed: serde_json::Value = serde_json::from_str(&written).expect("valid JSON");
        assert_eq!(parsed["openapi"], "3.1.0");
        assert!(parsed["paths"]["/users/{id}"]["get"].is_object());
        assert!(io.stdout.contains("openapi"), "stdout: {}", io.stdout);
    }

    /// An empty route table still writes a valid document with empty paths.
    #[tokio::test]
    async fn empty_registry_writes_a_valid_document() {
        let _guard = ROUTE_LOCK.lock().await;
        crate::routes::clear_route_source();

        let output = temp_output();
        let mut io = Io::default();
        OpenApiGenerate
            .run(vec![format!("--output={}", output.display())], &mut io)
            .await
            .expect("openapi:generate runs");

        let written = std::fs::read_to_string(&output).expect("spec file written");
        let _ = std::fs::remove_file(&output);

        let parsed: serde_json::Value = serde_json::from_str(&written).expect("valid JSON");
        assert_eq!(parsed["openapi"], "3.1.0");
        assert_eq!(parsed["paths"].as_object().expect("paths").len(), 0);
    }

    /// `--output` parses both `=` and space-separated forms; the default is
    /// `openapi.json` when absent.
    #[test]
    fn output_path_parsing() {
        let eq = vec!["--output=docs.json".to_string()];
        assert_eq!(output_path(&eq), "docs.json");

        let spaced = vec!["--output".to_string(), "docs.json".to_string()];
        assert_eq!(output_path(&spaced), "docs.json");

        let absent: Vec<String> = Vec::new();
        assert_eq!(output_path(&absent), DEFAULT_OUTPUT);

        let bare = vec!["--output".to_string()];
        assert_eq!(output_path(&bare), DEFAULT_OUTPUT);
    }

    /// An unsupported method in the registry surfaces as a typed CLI error.
    #[tokio::test]
    async fn unsupported_method_is_a_typed_error() {
        let _guard = ROUTE_LOCK.lock().await;
        crate::routes::set_routes(vec![entry("BREW", "/coffee")]);

        let output = temp_output();
        let mut io = Io::default();
        let result = OpenApiGenerate
            .run(vec![format!("--output={}", output.display())], &mut io)
            .await;
        crate::routes::clear_route_source();

        assert!(
            matches!(result, Err(CliError::Domain(_))),
            "expected Domain error, got {result:?}"
        );
        assert!(!output.exists(), "no file must be written on failure");
    }
}
