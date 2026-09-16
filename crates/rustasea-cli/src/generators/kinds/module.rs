//! `make:module` template — `modules/<snake>/` workspace crate (ADOPT-027).
//!
//! Scaffolds a self-contained module crate implementing the runtime
//! `rustasea::modules::Module` contract: the Cargo manifest, `src/lib.rs` (the
//! trait impl), `src/routes.rs`, `src/providers/`, `config/<name>.toml`, and a
//! `migrations/` directory. The `modules/*` workspace member glob emitted by
//! `cargo rustasea new --modular` wires the crate into the application build.

use std::path::Path;

use crate::error::CliResult;
use crate::generator::Generated;
use crate::generators::kinds::{slug, write_scaffold};
use crate::generators::MakeOptions;

/// Render and write every file of the module crate.
///
/// Unlike class generators this writes a whole crate, so it returns the full
/// file list in write order.
pub fn scaffold(root: &Path, opts: &MakeOptions) -> CliResult<Vec<Generated>> {
    let slug = slug(&opts.name);
    let files = [
        (
            format!("modules/{slug}/Cargo.toml"),
            cargo_toml(root, &slug, &opts.name),
        ),
        (
            format!("modules/{slug}/src/lib.rs"),
            lib_rs(&slug, &opts.name),
        ),
        (format!("modules/{slug}/src/routes.rs"), routes_rs(&slug)),
        (
            format!("modules/{slug}/src/providers/mod.rs"),
            providers_rs(&slug),
        ),
        (
            format!("modules/{slug}/config/{slug}.toml"),
            config_toml(&slug),
        ),
        (
            format!("modules/{slug}/migrations/.gitkeep"),
            migrations_marker(&slug),
        ),
    ];

    let mut written = Vec::with_capacity(files.len());
    for (relative_path, source) in files {
        written.push(write_scaffold(root, relative_path, source, opts.force)?);
    }
    Ok(written)
}

/// The module's Cargo manifest.
fn cargo_toml(root: &Path, slug: &str, name: &str) -> String {
    let dependency = framework_dependency(root);
    format!(
        r#"# {name} module crate — part of the modular application layout.
#
# The application workspace lists `modules/*` as members, so this crate builds
# with the application; the module is mounted by the `ModuleRegistry`.

[package]
name = "module-{slug}"
version = "0.1.0"
edition = "2021"
license = "MIT"

[dependencies]
{dependency}
"#
    )
}

/// The umbrella dependency line matching the surrounding project layout.
///
/// A framework source checkout exposes the umbrella at `crates/rustasea`
/// relative to the application root; a generated application consumes the
/// published crate instead. Only the always-available `modules` feature is
/// requested — routing and the ORM are core to the umbrella.
fn framework_dependency(root: &Path) -> &'static str {
    if root.join("crates/rustasea/Cargo.toml").is_file() {
        r#"rustasea = { path = "../../crates/rustasea", features = ["modules"] }"#
    } else {
        r#"rustasea = { version = "0.1", features = ["modules"] }"#
    }
}

/// The module's `lib.rs` implementing the runtime `Module` contract.
fn lib_rs(slug: &str, name: &str) -> String {
    format!(
        r#"//! {name} module — routes, providers, and migrations.
//!
//! Implements the runtime `Module` contract so the application's
//! `ModuleRegistry` mounts this module's routes, providers, and migrations in
//! deterministic order.

pub mod providers;
pub mod routes;

use rustasea::modules::Module;
use rustasea::router::Router;

/// The {name} module.
pub struct {name};

impl Module for {name} {{
    /// Unique module name (matches the `modules/{slug}` directory).
    fn name(&self) -> &str {{
        "{slug}"
    }}

    /// Module version reported by `module:list`.
    fn version(&self) -> &str {{
        env!("CARGO_PKG_VERSION")
    }}

    /// Register the module's HTTP routes.
    fn routes(&self, router: &mut Router) {{
        routes::register(router);
    }}
}}
"#
    )
}

/// The module's route registration entry point.
fn routes_rs(slug: &str) -> String {
    format!(
        r#"//! {slug} module HTTP routes.

use rustasea::router::Router;

/// Register the module's routes on `router`.
pub fn register(router: &mut Router) {{
    router.get("/{slug}");
}}
"#
    )
}

/// The module's provider module.
fn providers_rs(slug: &str) -> String {
    format!(
        r#"//! {slug} module service providers.
//!
//! Add providers here and return them from the module's `providers` hook.

// TODO: implement module service providers (rustasea::ServiceProvider).
"#
    )
}

/// The module's configuration file.
fn config_toml(slug: &str) -> String {
    format!(
        r#"# {slug} module configuration.
#
# Discovered by `module:list`; add module-specific settings below.

[module]
version = "0.1.0"
"#
    )
}

/// Placeholder keeping the (initially empty) migrations directory in VCS.
fn migrations_marker(slug: &str) -> String {
    format!("# {slug} module migrations belong in this directory.\n")
}
