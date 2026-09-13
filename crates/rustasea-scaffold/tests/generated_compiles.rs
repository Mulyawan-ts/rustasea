//! Generated-output compile gate (AUTH-001).
//!
//! The existing scaffold tests assert on template *strings*, which is exactly
//! why template drift (`Validatable::rules`, the fluent `Rules` builder,
//! `impl FormRequest for …`, the `tower_sessions` path) shipped undetected.
//! This suite closes that gap by rendering every variant through the public
//! [`Scaffold`] API and compiling the result with a real `cargo check`.
//!
//! # Strategy: real `cargo check`, not `syn` parsing
//!
//! A `syn`-parse pass only proves the generated files are *syntactically* valid
//! Rust. The regression this gate must catch — `impl FormRequest for X {}` — is
//! perfectly valid syntax (`FormRequest` simply resolves to a struct at the
//! type-check stage), so a parser would pass it. Only a real type check catches
//! it. Each variant is therefore generated into a scratch crate whose
//! `rustasea*` dependencies are redirected to this workspace via
//! `[patch.crates-io]`, then checked with `cargo check --all-targets`.
//!
//! # Environment note (why a dedicated target dir)
//!
//! A full `cargo check` fails spuriously when `CARGO_TARGET_DIR` lands under
//! `/tmp`: on this machine `/tmp` is a small tmpfs that fills up, and the
//! linker dies with `ld terminated with signal 7 [Bus error]` /
//! `No space left on device`. The gate therefore writes its scratch tree and
//! build artefacts under the workspace `target/` directory (git-ignored, on the
//! large root filesystem) instead of the system temp dir.
//!
//! # Cost
//!
//! The first run cold-compiles the generated app's dependency tree (a few
//! minutes); later variants reuse the same target dir and check in seconds.
//! All four variants share one target dir so the cost is paid once.

use std::path::{Path, PathBuf};
use std::process::Command;

use rustasea_scaffold::{Scaffold, StarterKitVariant};

/// Absolute path to the workspace root (two levels above this crate).
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate lives at <root>/crates/rustasea-scaffold")
        .to_path_buf()
}

/// Scratch root for the gate, under the workspace `target/` (git-ignored and on
/// the large filesystem — see the module docs on the `/tmp` tmpfs limitation).
fn scratch_root() -> PathBuf {
    workspace_root()
        .join("target")
        .join("scaffold-generated-gate")
}

/// Read the generated `Cargo.toml`'s direct `rustasea*` dependencies.
///
/// Parsing the manifest (rather than hard-coding the list) keeps the gate
/// honest: if a template starts depending on a new workspace crate, the gate
/// patches it automatically instead of failing on an unpatched registry fetch.
fn rustasea_dependencies(manifest: &str) -> Vec<String> {
    let mut deps = Vec::new();
    let mut in_dependencies = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_dependencies = trimmed == "[dependencies]";
            continue;
        }
        if !in_dependencies || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((name, _)) = trimmed.split_once('=') {
            let name = name.trim();
            if name == "rustasea" || name.starts_with("rustasea-") {
                deps.push(name.to_string());
            }
        }
    }
    deps
}

/// Append `[workspace]` (detach from the parent workspace) and a
/// `[patch.crates-io]` block redirecting every `rustasea*` dependency to the
/// local source tree.
///
/// The generated manifest keeps its published `version = "0.1"` requirements
/// and feature lists — the patch only changes *where* those crates resolve
/// from. That matters: feature drift (e.g. requesting a non-existent
/// `broadcast` feature) still fails resolution and is caught.
fn patch_manifest(manifest_path: &Path, deps: &[String]) {
    let root = workspace_root();
    let mut patched = std::fs::read_to_string(manifest_path).expect("read generated manifest");
    patched.push_str("\n[workspace]\n\n[patch.crates-io]\n");
    for dep in deps {
        patched.push_str(&format!(
            "{dep} = {{ path = \"{}\" }}\n",
            root.join("crates").join(dep).display()
        ));
    }
    std::fs::write(manifest_path, patched).expect("write patched manifest");
}

/// The `cargo` binary to drive, as reported by the running cargo.
fn cargo_bin() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

/// Generate `variant` into a fresh scratch crate and type-check it.
///
/// Returns `Ok(())` when `cargo check --all-targets` succeeds; on failure
/// returns the captured compiler diagnostics.
fn check_variant(variant: StarterKitVariant) -> Result<(), String> {
    let app_dir = scratch_root().join(variant.as_str());
    // Fresh tree every run so `Scaffold::generate` never trips `AlreadyExists`
    // and no stale source survives a template change.
    if app_dir.exists() {
        std::fs::remove_dir_all(&app_dir).map_err(|e| format!("clean scratch tree: {e}"))?;
    }
    std::fs::create_dir_all(&app_dir).map_err(|e| format!("create scratch tree: {e}"))?;

    Scaffold::new("gate-app", variant)
        .generate(&app_dir)
        .map_err(|e| format!("generate {variant}: {e}"))?;

    let manifest_path = app_dir.join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path).expect("read Cargo.toml");
    patch_manifest(&manifest_path, &rustasea_dependencies(&manifest));

    // A dedicated target dir keeps the nested cargo invocation off the outer
    // `cargo test` target lock (which would otherwise deadlock) and is shared
    // across variants so dependencies compile once.
    let target_dir = scratch_root().join("target");
    let output = Command::new(cargo_bin())
        .arg("check")
        .arg("--manifest-path")
        .arg(&manifest_path)
        .arg("--all-targets")
        .arg("--offline")
        .env("CARGO_TARGET_DIR", &target_dir)
        .output()
        .map_err(|e| format!("spawn cargo check for {variant}: {e}"))?;

    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Surface only the actionable diagnostics (error lines + their locations).
    let diagnostics: String = stderr
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            line.starts_with("error") || line.starts_with("-->")
        })
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!(
        "generated `{variant}` app failed `cargo check`:\n{diagnostics}\n\nfull output:\n{stderr}"
    ))
}

/// Every variant's generated tree type-checks against the real workspace.
///
/// This is the primary regression guard: a single broken construct in any
/// shared or variant template (the request objects, the `tower_sessions` path,
/// the ORM model, the manifests) fails this test with the exact rustc error.
#[test]
fn generated_app_compiles_for_every_variant() {
    let mut failures = Vec::new();
    for variant in StarterKitVariant::ALL {
        if let Err(error) = check_variant(variant) {
            failures.push(error);
        }
    }
    assert!(
        failures.is_empty(),
        "generated output must compile for every variant:\n\n{}",
        failures.join("\n\n")
    );
}

/// Fast, always-on companion guard for the specific AUTH-001 drift.
///
/// The compile gate above is authoritative but slow; this cheap string check
/// fails immediately (no toolchain) when one of the removed constructs
/// reappears, so a regression is caught even before the slow gate runs.
#[test]
fn generated_requests_avoid_the_removed_constructs() {
    for variant in StarterKitVariant::ALL {
        let files = Scaffold::new("gate-app", variant).render().expect("render");
        let find = |path: &str| {
            files
                .iter()
                .find(|file| file.path == path)
                .unwrap_or_else(|| panic!("missing {path} ({variant})"))
                .contents
                .as_str()
        };

        for request in [
            "app/http/requests/settings/profile_update_request.rs",
            "app/http/requests/settings/password_update_request.rs",
        ] {
            let body = find(request);
            assert!(
                !body.contains("impl FormRequest for"),
                "{request} must not `impl FormRequest` — it is a struct, not a trait ({variant})"
            );
            assert!(
                !body.contains("fn rules()"),
                "{request} must not declare `fn rules()` — `Validatable` has no such method ({variant})"
            );
            assert!(
                !body.contains(".required(")
                    && !body.contains(".max(")
                    && !body.contains(".email("),
                "{request} must not call the non-existent fluent rule methods ({variant})"
            );
            assert!(
                body.contains("fn validate(&self)"),
                "{request} must implement `Validatable::validate` ({variant})"
            );
        }

        let attempt = find("app/actions/auth/attempt_to_authenticate.rs");
        if attempt.contains("tower_sessions::") {
            // The template names the `tower_sessions` crate directly, so the
            // generated manifest must declare the dependency (otherwise the
            // path is unlinked — the original AUTH-001 blocker).
            let manifest = find("Cargo.toml");
            assert!(
                manifest.contains("tower-sessions = "),
                "a template referencing `tower_sessions::` requires a `tower-sessions` \
                 dependency in Cargo.toml ({variant})"
            );
        }
    }
}
