//! Modular application commands — `module:list`, `module:enable`,
//! `module:disable` (ADOPT-027, `nwidart/laravel-modules` parity).
//!
//! The commands operate on the on-disk module layout: `modules/<name>/` is
//! discovered through [`rustasea_modules::discover_modules`] and the
//! enable/disable state lives in the `[modules]` table of `rustasea.toml`
//! (or `config/modules.toml` when present), written back by
//! [`rustasea_modules::ModuleManifest::save`].

use async_trait::async_trait;
use serde::Serialize;

use crate::artisan::{Command, Io};
use crate::error::{project_root, CliError, CliResult};
use crate::output;
use rustasea_modules::{discover_modules, ModuleDescriptor, ModuleManifest};

/// One row of `module:list --json` output.
#[derive(Debug, Serialize)]
struct ModuleEntry {
    /// Module name (directory basename under `modules/`).
    name: String,
    /// `enabled` or `disabled`.
    status: String,
    /// Version declared by the module's `config/<name>.toml`, when present.
    version: Option<String>,
    /// Workspace-relative path to the module crate root.
    path: String,
}

/// Builds a [`ModuleEntry`] from a discovered descriptor.
fn entry(descriptor: &ModuleDescriptor) -> ModuleEntry {
    ModuleEntry {
        name: descriptor.name.clone(),
        status: if descriptor.enabled {
            "enabled".into()
        } else {
            "disabled".into()
        },
        version: descriptor.version.clone(),
        path: descriptor.path.display().to_string(),
    }
}

/// Resolves the application root and the current module manifest.
///
/// Discovery is intentionally part of the loading step: both the table and the
/// enable/disable guards need the set of known modules, and both need the
/// manifest that decides their state.
fn load(root: &std::path::Path) -> CliResult<(ModuleManifest, Vec<ModuleDescriptor>)> {
    let manifest =
        ModuleManifest::read(root).map_err(|error| CliError::Domain(error.to_string()))?;
    let modules =
        discover_modules(root, &manifest).map_err(|error| CliError::Domain(error.to_string()))?;
    Ok((manifest, modules))
}

/// Extracts the first positional (non-flag) argument.
fn positional(args: &[String]) -> Option<String> {
    args.iter()
        .find(|arg| !arg.starts_with('-'))
        .filter(|arg| !arg.is_empty())
        .cloned()
}

/// `module:list` — list discovered modules with their enable/disable state.
pub struct ModuleList;

#[async_trait]
impl Command for ModuleList {
    /// Command signature.
    fn signature(&self) -> &'static str {
        "module:list"
    }

    /// Usage line rendered by `list`.
    fn usage(&self) -> Option<&'static str> {
        Some("module:list [--json]")
    }

    /// One-line help rendered by `list`.
    fn help(&self) -> Option<&'static str> {
        Some("List application modules and their status")
    }

    /// Execute: resolve the project root and list its modules.
    async fn run(&self, args: Vec<String>, io: &mut Io) -> CliResult<()> {
        let json = args.iter().any(|arg| arg == "--json");
        let root = project_root(&std::env::current_dir()?)?;
        list_at(&root, json, io)
    }
}

/// Lists the modules discovered under `root` (the testable core of the command).
fn list_at(root: &std::path::Path, json: bool, io: &mut Io) -> CliResult<()> {
    let (_manifest, modules) = load(root)?;

    if modules.is_empty() {
        if json {
            io.line("[]");
        } else {
            io.line("No modules found. Run `make:module <name>` to create one.");
        }
        return Ok(());
    }

    let entries: Vec<ModuleEntry> = modules.iter().map(entry).collect();
    if json {
        io.line(serde_json::to_string(&entries)?);
        return Ok(());
    }

    let mut rows = vec![vec![
        "Module".to_string(),
        "Status".to_string(),
        "Version".to_string(),
        "Path".to_string(),
    ]];
    for item in &entries {
        rows.push(vec![
            item.name.clone(),
            item.status.clone(),
            item.version.clone().unwrap_or_else(|| "-".into()),
            item.path.clone(),
        ]);
    }
    io.line(output::table(rows).trim_end());
    Ok(())
}

/// `module:enable` — mark an existing module as enabled.
pub struct ModuleEnable;

#[async_trait]
impl Command for ModuleEnable {
    /// Command signature.
    fn signature(&self) -> &'static str {
        "module:enable"
    }

    /// Usage line rendered by `list`.
    fn usage(&self) -> Option<&'static str> {
        Some("module:enable {name}")
    }

    /// One-line help rendered by `list`.
    fn help(&self) -> Option<&'static str> {
        Some("Enable an application module")
    }

    /// Execute: resolve the project root and enable the named module.
    async fn run(&self, args: Vec<String>, io: &mut Io) -> CliResult<()> {
        let root = project_root(&std::env::current_dir()?)?;
        set_enabled_at("module:enable", true, &root, &args, io)
    }
}

/// `module:disable` — mark an existing module as disabled.
pub struct ModuleDisable;

#[async_trait]
impl Command for ModuleDisable {
    /// Command signature.
    fn signature(&self) -> &'static str {
        "module:disable"
    }

    /// Usage line rendered by `list`.
    fn usage(&self) -> Option<&'static str> {
        Some("module:disable {name}")
    }

    /// One-line help rendered by `list`.
    fn help(&self) -> Option<&'static str> {
        Some("Disable an application module")
    }

    /// Execute: resolve the project root and disable the named module.
    async fn run(&self, args: Vec<String>, io: &mut Io) -> CliResult<()> {
        let root = project_root(&std::env::current_dir()?)?;
        set_enabled_at("module:disable", false, &root, &args, io)
    }
}

/// Persists an enable/disable transition at `root` (testable core).
///
/// The manifest is only written after the module name is confirmed to exist on
/// disk, so a typo can never leave a dangling entry behind.
fn set_enabled_at(
    command: &str,
    enable: bool,
    root: &std::path::Path,
    args: &[String],
    io: &mut Io,
) -> CliResult<()> {
    let (mut manifest, modules) = load(root)?;
    let name = require_known(command, positional(args), &modules)?;
    if enable {
        manifest.enable(&name);
    } else {
        manifest.disable(&name);
    }
    manifest
        .save()
        .map_err(|error| CliError::Domain(error.to_string()))?;
    let verb = if enable { "enabled" } else { "disabled" };
    io.line(format!("{verb}: {name}"));
    io.line(format!("manifest: {}", manifest.path().display()));
    Ok(())
}

/// Resolves a required module name and rejects names that are not on disk.
///
/// Enabling or disabling a typo would silently produce a manifest entry that
/// never matches a crate, so the guard fails loudly with the known names.
fn require_known(
    command: &str,
    name: Option<String>,
    modules: &[ModuleDescriptor],
) -> CliResult<String> {
    let name = name.ok_or_else(|| CliError::InvalidArguments {
        command: command.into(),
        detail: "missing module name".into(),
    })?;

    if modules.iter().any(|module| module.name == name) {
        return Ok(name);
    }

    let known: Vec<&str> = modules.iter().map(|module| module.name.as_str()).collect();
    let hint = if known.is_empty() {
        "no modules exist yet".to_string()
    } else {
        format!("known modules: {}", known.join(", "))
    };
    Err(CliError::InvalidArguments {
        command: command.into(),
        detail: format!("unknown module `{name}` ({hint})"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Builds a throwaway project with two discoverable module crates.
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (name, version) in [("blog", "0.2.0"), ("shop", "0.3.0")] {
            let module = dir.path().join("modules").join(name);
            fs::create_dir_all(module.join("config")).expect("module dirs");
            fs::write(module.join("Cargo.toml"), "[package]\nname = \"module\"\n")
                .expect("module manifest");
            fs::write(
                module.join("config").join(format!("{name}.toml")),
                format!("[module]\nversion = \"{version}\"\n"),
            )
            .expect("module config");
        }
        dir
    }

    /// `module:list` renders each module with status and version.
    #[test]
    fn list_reports_status_and_version() {
        let dir = fixture();
        let mut io = Io::default();
        list_at(dir.path(), false, &mut io).expect("list succeeds");
        assert!(io.stdout.contains("blog"), "stdout: {}", io.stdout);
        assert!(io.stdout.contains("enabled"), "stdout: {}", io.stdout);
        assert!(io.stdout.contains("0.2.0"), "stdout: {}", io.stdout);
        assert!(io.stdout.contains("modules/blog"), "stdout: {}", io.stdout);
    }

    /// `module:list --json` emits a machine-readable, name-sorted array.
    #[test]
    fn list_json_is_sorted_and_machine_readable() {
        let dir = fixture();
        let mut io = Io::default();
        list_at(dir.path(), true, &mut io).expect("list succeeds");
        let value: serde_json::Value = serde_json::from_str(io.stdout.trim()).expect("valid json");
        let names: Vec<&str> = value
            .as_array()
            .expect("array")
            .iter()
            .map(|item| item["name"].as_str().expect("name"))
            .collect();
        assert_eq!(names, vec!["blog", "shop"]);
        assert_eq!(value[0]["status"], "enabled");
    }

    /// Disabling a module persists the state and flips its reported status.
    #[test]
    fn disable_persists_and_excludes_from_enabled() {
        let dir = fixture();
        let args = vec!["blog".to_string()];
        let mut io = Io::default();
        set_enabled_at("module:disable", false, dir.path(), &args, &mut io).expect("disable");
        assert!(
            io.stdout.contains("disabled: blog"),
            "stdout: {}",
            io.stdout
        );

        let manifest = ModuleManifest::read(dir.path()).expect("manifest");
        assert!(!manifest.is_enabled("blog"));
        assert!(manifest.is_enabled("shop"));

        let mut listed = Io::default();
        list_at(dir.path(), false, &mut listed).expect("list succeeds");
        let blog_line = listed
            .stdout
            .lines()
            .find(|line| line.contains("blog"))
            .expect("blog row");
        assert!(blog_line.contains("disabled"), "row: {blog_line}");
    }

    /// Enable/disable round-trips through the manifest without losing the name.
    #[test]
    fn enable_after_disable_restores_module() {
        let dir = fixture();
        let args = vec!["blog".to_string()];
        let mut io = Io::default();
        set_enabled_at("module:disable", false, dir.path(), &args, &mut io).expect("disable");
        let mut again = Io::default();
        set_enabled_at("module:enable", true, dir.path(), &args, &mut again).expect("enable");
        assert!(
            again.stdout.contains("enabled: blog"),
            "stdout: {}",
            again.stdout
        );

        let manifest = ModuleManifest::read(dir.path()).expect("manifest");
        assert!(manifest.is_enabled("blog"));
    }

    /// Unknown module names are rejected before the manifest is written.
    #[test]
    fn unknown_module_is_rejected() {
        let dir = fixture();
        let args = vec!["nope".to_string()];
        let mut io = Io::default();
        let error = set_enabled_at("module:enable", true, dir.path(), &args, &mut io)
            .expect_err("unknown module rejected");
        match error {
            CliError::InvalidArguments { command, detail } => {
                assert_eq!(command, "module:enable");
                assert!(detail.contains("unknown module `nope`"), "detail: {detail}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(!dir.path().join("config").join("modules.toml").exists());
    }

    /// A missing positional name is an argument error.
    #[test]
    fn missing_module_name_is_rejected() {
        let dir = fixture();
        let mut io = Io::default();
        let error = set_enabled_at("module:disable", false, dir.path(), &[], &mut io)
            .expect_err("missing name rejected");
        match error {
            CliError::InvalidArguments { detail, .. } => {
                assert!(detail.contains("missing module name"), "detail: {detail}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
