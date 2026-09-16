//! Workspace dependency drift detection for `cargo xtask deps:check`.
//!
//! The root `Cargo.toml` owns a `[workspace.dependencies]` table that is the
//! single source of truth for every shared crate version (ADOPT-031). This
//! module fails CI when a member manifest (`crates/*/Cargo.toml`,
//! `xtask/Cargo.toml`) pins a version inline for a crate the workspace already
//! manages — either as a bare string (`reqwest = "0.12"`) or as a table with a
//! `version` key but no `workspace = true`.
//!
//! Manifests are parsed with `toml_edit` rather than `toml` because its items
//! retain their source span, so a violation is reported at its exact
//! `file:line`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{ImDocument, Item, Table, Value};

use crate::FAILURE;

/// Dependency tables scanned in every member manifest.
const DEPENDENCY_TABLES: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];

/// A member dependency that pins a version inline instead of inheriting it.
struct Violation {
    /// Name of the offending dependency.
    dependency: String,
    /// Manifest table path, e.g. `dependencies` or
    /// `target.cfg(unix).dependencies`.
    table: String,
    /// Human-readable description of what was declared.
    declared: String,
    /// 1-based source line of the declaration.
    line: usize,
}

/// Run the drift check across every workspace member manifest.
///
/// Prints each violation as `path:line` and returns [`FAILURE`] when any member
/// pins a workspace-managed crate inline; returns `0` when every member
/// inherits the workspace declaration.
pub fn run() -> i32 {
    let root = workspace_root();
    let root_manifest = root.join("Cargo.toml");
    let root_source = match fs::read_to_string(&root_manifest) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("xtask: cannot read {}: {error}", root_manifest.display());
            return FAILURE;
        }
    };
    // `ImDocument` (not `DocumentMut`) is used so source spans survive: the
    // mutable form despans its items on conversion.
    let root_document = match ImDocument::parse(root_source.as_str()) {
        Ok(document) => document,
        Err(error) => {
            eprintln!("xtask: cannot parse {}: {error}", root_manifest.display());
            return FAILURE;
        }
    };
    let managed = workspace_dependency_names(root_document.as_table());

    let manifests = member_manifests(&root);
    let mut violations: Vec<(PathBuf, Violation)> = Vec::new();
    for manifest in &manifests {
        let source = match fs::read_to_string(manifest) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("xtask: cannot read {}: {error}", manifest.display());
                return FAILURE;
            }
        };
        match scan_manifest(&source, &managed) {
            Ok(found) => violations.extend(
                found
                    .into_iter()
                    .map(|violation| (manifest.clone(), violation)),
            ),
            Err(error) => {
                eprintln!("xtask: cannot parse {}: {error}", manifest.display());
                return FAILURE;
            }
        }
    }

    if violations.is_empty() {
        println!(
            "xtask: all member dependencies consolidated ({} crates checked).",
            manifests.len()
        );
        return 0;
    }

    for (manifest, violation) in &violations {
        eprintln!(
            "{}:{}: `{}` declares {} (table `{}`); use `workspace = true`.",
            manifest.display(),
            violation.line,
            violation.dependency,
            violation.declared,
            violation.table
        );
    }
    eprintln!(
        "xtask: {} inline declaration(s) drift from [workspace.dependencies].",
        violations.len()
    );
    FAILURE
}

/// Resolve the workspace root from this binary's compile-time manifest path.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Collect member manifests: every `crates/*/Cargo.toml` plus `xtask/Cargo.toml`.
fn member_manifests(root: &Path) -> Vec<PathBuf> {
    let mut manifests: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(root.join("crates")) {
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        dirs.sort();
        for dir in dirs {
            let manifest = dir.join("Cargo.toml");
            if manifest.is_file() {
                manifests.push(manifest);
            }
        }
    }
    let xtask = root.join("xtask").join("Cargo.toml");
    if xtask.is_file() {
        manifests.push(xtask);
    }
    manifests
}

/// Extract the keys of the root `[workspace.dependencies]` table.
fn workspace_dependency_names(root: &Table) -> BTreeSet<String> {
    root.get("workspace")
        .and_then(Item::as_table)
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(Item::as_table)
        .map(|dependencies| {
            dependencies
                .iter()
                .map(|(name, _)| name.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Scan one manifest source for inline versions of workspace-managed crates.
///
/// Covers the three dependency tables at the root and under every
/// `[target.<cfg>.*]` table. Returns a parse error when the source is not valid
/// TOML.
fn scan_manifest(source: &str, managed: &BTreeSet<String>) -> Result<Vec<Violation>, String> {
    let document =
        ImDocument::parse(source).map_err(|error: toml_edit::TomlError| error.to_string())?;
    let root = document.as_table();
    let mut violations = Vec::new();

    for table_name in DEPENDENCY_TABLES {
        if let Some(table) = root.get(table_name).and_then(Item::as_table) {
            collect(table, table_name, source, managed, &mut violations);
        }
    }

    if let Some(targets) = root.get("target").and_then(Item::as_table) {
        for (target, item) in targets.iter() {
            let Some(target_table) = item.as_table() else {
                continue;
            };
            for table_name in DEPENDENCY_TABLES {
                if let Some(table) = target_table.get(table_name).and_then(Item::as_table) {
                    let path = format!("target.{target}.{table_name}");
                    collect(table, &path, source, managed, &mut violations);
                }
            }
        }
    }

    Ok(violations)
}

/// Test one dependency table, appending a violation per inline version.
fn collect(
    table: &Table,
    path: &str,
    source: &str,
    managed: &BTreeSet<String>,
    violations: &mut Vec<Violation>,
) {
    for (name, item) in table.iter() {
        if !managed.contains(name) {
            continue;
        }
        let Some(declared) = inline_version(item) else {
            continue;
        };
        let line = table
            .get_key_value(name)
            .and_then(|(key, _)| key.span())
            .map(|span| line_of(source, span.start))
            .unwrap_or(0);
        violations.push(Violation {
            dependency: name.to_string(),
            table: path.to_string(),
            declared,
            line,
        });
    }
}

/// Describe the inline version a dependency entry declares, if any.
///
/// Returns `None` for a path/git dependency and for an entry that inherits the
/// workspace declaration (`workspace = true`).
fn inline_version(item: &Item) -> Option<String> {
    if let Some(value) = item.as_value() {
        if let Some(version) = value.as_str() {
            return Some(format!("version = {version:?}"));
        }
        let table = value.as_inline_table()?;
        return declared_version(
            table.get("workspace").and_then(Value::as_bool) == Some(true),
            table.get("version").and_then(Value::as_str),
        );
    }
    let table = item.as_table()?;
    declared_version(
        table
            .get("workspace")
            .and_then(Item::as_value)
            .and_then(Value::as_bool)
            == Some(true),
        table
            .get("version")
            .and_then(Item::as_value)
            .and_then(Value::as_str),
    )
}

/// Build the violation text when a version is pinned without `workspace = true`.
fn declared_version(inherits_workspace: bool, version: Option<&str>) -> Option<String> {
    if inherits_workspace {
        return None;
    }
    version.map(|version| format!("version = {version:?} without `workspace = true`"))
}

/// Convert a byte offset in `source` to a 1-based line number.
fn line_of(source: &str, offset: usize) -> usize {
    let end = offset.min(source.len());
    source
        .get(..end)
        .map(|prefix| prefix.bytes().filter(|byte| *byte == b'\n').count() + 1)
        .unwrap_or_else(|| source.lines().count().max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the managed-crate set used by the fixture manifests.
    fn managed() -> BTreeSet<String> {
        ["serde", "reqwest", "http", "tower", "tokio", "tempfile"]
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    /// Runtime and dev tables flag inline versions; inherited entries do not.
    #[test]
    fn flags_inline_versions_across_tables() {
        let fixture = "\
[package]
name = \"demo\"

[dependencies]
serde = { workspace = true }
reqwest = { version = \"0.12\", features = [\"json\"] }
http = \"1\"
anyhow = \"1\"
tower = { workspace = true, features = [\"util\"] }

[dev-dependencies]
tokio = { version = \"1\", features = [\"macros\"] }
";
        let violations = scan_manifest(fixture, &managed()).expect("parse fixture");
        let found: Vec<(&str, usize)> = violations
            .iter()
            .map(|violation| (violation.dependency.as_str(), violation.line))
            .collect();
        assert_eq!(found, vec![("reqwest", 6), ("http", 7), ("tokio", 12)]);
    }

    /// Target tables and `[dependencies.foo]` subtables are also scanned.
    #[test]
    fn flags_target_and_subtable_forms() {
        let fixture = "\
[dependencies.reqwest]
version = \"0.12\"

[target.'cfg(unix)'.dependencies]
tempfile = { version = \"3\" }

[target.'cfg(unix)'.dev-dependencies]
tokio = { workspace = true, features = [\"macros\"] }
";
        let violations = scan_manifest(fixture, &managed()).expect("parse fixture");
        let found: Vec<(&str, &str, usize)> = violations
            .iter()
            .map(|violation| {
                (
                    violation.dependency.as_str(),
                    violation.table.as_str(),
                    violation.line,
                )
            })
            .collect();
        assert_eq!(
            found,
            vec![
                ("reqwest", "dependencies", 1),
                ("tempfile", "target.cfg(unix).dependencies", 5),
            ]
        );
    }

    /// A manifest that fully inherits workspace deps reports no drift.
    #[test]
    fn fully_inherited_manifest_is_clean() {
        let fixture = "\
[dependencies]
serde = { workspace = true }
reqwest = { workspace = true, features = [\"stream\"] }
local = { path = \"../local\" }

[features]
extra = [\"reqwest/stream\"]
";
        assert!(scan_manifest(fixture, &managed())
            .expect("parse fixture")
            .is_empty());
    }

    /// The root `[workspace.dependencies]` keys become the managed set.
    #[test]
    fn reads_workspace_dependency_names() {
        let root = "\
[workspace]
members = [\"crates/*\"]

[workspace.dependencies]
serde = \"1\"
reqwest = { version = \"0.12\", default-features = false }
";
        let document = ImDocument::parse(root).expect("parse root");
        let names = workspace_dependency_names(document.as_table());
        assert_eq!(
            names,
            ["reqwest".to_string(), "serde".to_string()]
                .into_iter()
                .collect()
        );
    }
}
