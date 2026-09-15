//! Docker compose helpers for the `cargo xtask docker:*` task surface.
//!
//! ADOPT-007 adds a container story with laravel/sail parity. These tasks wrap
//! the compose CLI so a developer can bring the stack up, follow its logs, and
//! tear it down without remembering the exact invocation.
//!
//! # Compose binary detection
//!
//! Compose ships two ways: the `docker compose` plugin (Docker CLI v2) and the
//! standalone `docker-compose` (v1) binary. Detection probes
//! `docker compose version` once per invocation; when the plugin is missing the
//! helpers fall back to `docker-compose`. The generated compose files declare a
//! `version: "3.8"` header so both binaries accept them.

use std::process::Command;

use crate::FAILURE;

/// A resolved compose invocation: the program plus its leading arguments.
struct Compose {
    /// Program to spawn (`docker` or `docker-compose`).
    program: String,
    /// Leading arguments (`["compose"]` for the plugin, empty for the binary).
    prefix: Vec<String>,
}

impl Compose {
    /// Detect the available compose front-end.
    ///
    /// Prefers the `docker compose` plugin; falls back to the standalone
    /// `docker-compose` binary when the plugin probe fails.
    fn detect() -> Self {
        let plugin_ok = Command::new("docker")
            .args(["compose", "version"])
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false);
        if plugin_ok {
            Compose {
                program: "docker".to_string(),
                prefix: vec!["compose".to_string()],
            }
        } else {
            Compose {
                program: "docker-compose".to_string(),
                prefix: Vec::new(),
            }
        }
    }

    /// Run the compose front-end with `args`, inheriting stdio.
    fn run(&self, args: &[String]) -> i32 {
        Command::new(&self.program)
            .args(&self.prefix)
            .args(args)
            .status()
            .map(|status| status.code().unwrap_or(FAILURE))
            .unwrap_or_else(|err| {
                eprintln!("xtask: failed to run {}: {err}", self.program);
                FAILURE
            })
    }
}

/// `cargo xtask docker:up` — start the stack detached and print service URLs.
pub fn up() -> i32 {
    let compose = Compose::detect();
    let code = compose.run(&["up".to_string(), "-d".to_string()]);
    if code == 0 {
        println!("xtask docker:up — stack is starting");
        println!("  app             http://localhost:8000");
        println!("  mailpit UI      http://localhost:8025");
        println!("  minio console   http://localhost:9001");
    }
    code
}

/// `cargo xtask docker:down` — stop and remove the stack.
pub fn down() -> i32 {
    Compose::detect().run(&["down".to_string()])
}

/// `cargo xtask docker:logs` — follow the stack logs.
///
/// Any remaining CLI arguments are forwarded to `logs` (e.g. a service name),
/// so `cargo xtask docker:logs app` follows just the app container.
pub fn logs(args: &[String]) -> i32 {
    let mut full: Vec<String> = vec!["logs".to_string(), "-f".to_string()];
    full.extend(args.iter().cloned());
    Compose::detect().run(&full)
}
