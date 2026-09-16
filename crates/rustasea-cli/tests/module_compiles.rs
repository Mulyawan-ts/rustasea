//! `make:module` compile gate (ADOPT-027).
//!
//! The string-level assertions in `make_module.rs` cannot catch drift between
//! the generator template and the real `rustasea::modules::Module` contract —
//! a renamed hook or a wrong import is valid syntax and only fails at
//! type-check time. This gate therefore scaffolds a module into a modular
//! workspace (the layout `cargo rustasea new --modular` emits) and runs a real
//! `cargo check`, so "the generated crate compiles and is wired into the
//! workspace" is proven by the compiler rather than by substring matching.
//!
//! # Environment note
//!
//! Like the scaffold compile gate, the scratch tree and its target dir live
//! under the workspace `target/` (git-ignored, large filesystem) because a
//! `CARGO_TARGET_DIR` under `/tmp` exhausts the tmpfs on this host. A dedicated
//! target dir also keeps the nested cargo invocation off the outer `cargo test`
//! target lock.

use std::path::{Path, PathBuf};
use std::process::Command;

use rustasea_cli::generators::{generate, Kind, MakeOptions};

/// Absolute path to the workspace root (two levels above this crate).
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate lives at <root>/crates/rustasea-cli")
        .to_path_buf()
}

/// Scratch root for the gate, under the workspace `target/`.
fn scratch_root() -> PathBuf {
    workspace_root().join("target").join("module-compile-gate")
}

/// The `cargo` binary to drive, as reported by the running cargo.
fn cargo_bin() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

/// Write the modular workspace root that admits `modules/*` as members.
///
/// `[patch.crates-io]` redirects the module's published `rustasea = "0.1"`
/// requirement to this source tree, so the gate checks the real umbrella (and
/// its `modules` feature) without a registry fetch.
fn write_workspace_root(app_dir: &Path) {
    let root = workspace_root();
    let manifest = format!(
        "# Modular application workspace emitted by `cargo rustasea new --modular`.\n\
         [workspace]\n\
         members = [\"modules/*\"]\n\
         resolver = \"2\"\n\n\
         [patch.crates-io]\n\
         rustasea = {{ path = \"{}\" }}\n",
        root.join("crates/rustasea").display()
    );
    std::fs::write(app_dir.join("Cargo.toml"), manifest).expect("write workspace root");
}

/// Scaffold `Blog` into a modular workspace and type-check the workspace.
fn check_module() -> Result<(), String> {
    let app_dir = scratch_root().join("app");
    if app_dir.exists() {
        std::fs::remove_dir_all(&app_dir).map_err(|e| format!("clean scratch tree: {e}"))?;
    }
    std::fs::create_dir_all(&app_dir).map_err(|e| format!("create scratch tree: {e}"))?;

    let options = MakeOptions {
        name: "Blog".to_string(),
        force: false,
        resource: false,
        with_migration: false,
        browser: false,
    };
    generate(Kind::Module, &app_dir, &options).map_err(|e| format!("generate module: {e}"))?;
    write_workspace_root(&app_dir);

    let output = Command::new(cargo_bin())
        .arg("check")
        .arg("--manifest-path")
        .arg(app_dir.join("Cargo.toml"))
        .arg("--all-targets")
        .arg("--offline")
        .env("CARGO_TARGET_DIR", scratch_root().join("target"))
        .output()
        .map_err(|e| format!("spawn cargo check: {e}"))?;

    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let diagnostics: String = stderr
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            line.starts_with("error") || line.starts_with("-->")
        })
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!(
        "generated module failed `cargo check`:\n{diagnostics}\n\nfull output:\n{stderr}"
    ))
}

/// The generated module crate compiles as a `modules/*` workspace member.
#[test]
fn generated_module_compiles_in_workspace() {
    if let Err(error) = check_module() {
        panic!("{error}");
    }
}
