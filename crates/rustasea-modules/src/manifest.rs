//! Module manifest state and on-disk module discovery.
//!
//! Enable/disable state lives in a `[modules]` table. The registry reads it
//! from (in order): `config/modules.toml`, then the application's
//! `rustasea.toml`. When neither declares a `[modules]` table the state is
//! empty and every discovered module stays enabled (the framework default).
//!
//! ```toml
//! [modules]
//! enabled = ["blog"]
//! disabled = ["admin"]
//! ```
//!
//! [`ModuleManifest::is_enabled`] resolves the two lists: an entry in
//! `disabled` always wins; an `enabled` key that is declared (even as `[]`)
//! acts as an allow-list; otherwise the module is enabled.
//!
//! [`ModuleManifest::save`] rewrites only the `[modules]` table through
//! `toml_edit`, so the rest of a user's `rustasea.toml` (other tables,
//! comments, key order) survives a `module:enable` / `module:disable`.

use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use toml_edit::{Array, DocumentMut, Item, Table, Value};

use crate::error::{ModuleError, ModuleResult};

/// Directory (relative to the application root) holding module crates.
pub const MODULES_DIR: &str = "modules";

/// Dedicated manifest path (relative to the application root).
pub const MODULES_MANIFEST: &str = "config/modules.toml";

/// Application manifest consulted when no dedicated manifest exists.
pub const APP_MANIFEST: &str = "rustasea.toml";

/// Raw `[modules]` table shape shared by both manifest locations.
///
/// `enabled` is an `Option` so the reader can tell an absent key (default-on
/// mode) from `enabled = []` (allow-list mode with nothing enabled).
#[derive(Debug, Clone, Default, Deserialize)]
struct ModulesTable {
    #[serde(default)]
    enabled: Option<Vec<String>>,
    #[serde(default)]
    disabled: Vec<String>,
}

/// Deserialization shim: both manifest files may carry a `[modules]` table.
#[derive(Debug, Default, Deserialize)]
struct ManifestDocument {
    #[serde(default)]
    modules: Option<ModulesTable>,
}

/// Deserialization shim for a per-module `config/<name>.toml`.
#[derive(Debug, Default, Deserialize)]
struct ModuleConfigDocument {
    #[serde(default)]
    module: Option<ModuleMetadata>,
}

/// Per-module metadata declared in `config/<name>.toml`.
#[derive(Debug, Default, Deserialize)]
struct ModuleMetadata {
    #[serde(default)]
    version: Option<String>,
}

/// A module crate discovered on disk under `modules/*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleDescriptor {
    /// Module name (the `modules/<name>` directory name).
    pub name: String,
    /// Workspace-relative module directory (`modules/blog`).
    pub path: PathBuf,
    /// Version declared in `config/<name>.toml`, when present.
    pub version: Option<String>,
    /// Whether the manifest currently enables the module.
    pub enabled: bool,
}

/// Persisted enable/disable state for the application's modules.
///
/// `disabled` is a deny-list; `enabled` is an allow-list that is only active
/// once the manifest declares the key. Keeping the two apart means
/// `module:enable` never silently narrows a default-on application, and an
/// allow-list that disables everything stays expressible as `enabled = []`.
#[derive(Debug, Clone)]
pub struct ModuleManifest {
    /// File the state is read from and written back to.
    path: PathBuf,
    /// Whether an `enabled` allow-list was declared.
    allow_list: bool,
    /// Explicitly enabled modules (allow-list entries).
    enabled: BTreeSet<String>,
    /// Explicitly disabled modules.
    disabled: BTreeSet<String>,
}

impl ModuleManifest {
    /// Read the manifest that governs `root`, defaulting to an empty state.
    ///
    /// Prefers `config/modules.toml`; falls back to a `[modules]` table in
    /// `rustasea.toml`; anchors future writes at `config/modules.toml` when
    /// neither exists.
    pub fn read(root: &Path) -> ModuleResult<Self> {
        let dedicated = root.join(MODULES_MANIFEST);
        if dedicated.is_file() {
            return Self::load(dedicated);
        }
        let app = root.join(APP_MANIFEST);
        if app.is_file() {
            let document = read_document(&app)?;
            if let Some(table) = document.modules {
                return Ok(Self::from_table(app, table));
            }
        }
        Ok(Self {
            path: dedicated,
            allow_list: false,
            enabled: BTreeSet::new(),
            disabled: BTreeSet::new(),
        })
    }

    /// Read a specific manifest file.
    pub fn load(path: impl Into<PathBuf>) -> ModuleResult<Self> {
        let path = path.into();
        let document = read_document(&path)?;
        Ok(Self::from_table(path, document.modules.unwrap_or_default()))
    }

    /// File the state is read from and written back to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Explicitly enabled module names (the allow-list; empty when inactive).
    pub fn enabled(&self) -> &BTreeSet<String> {
        &self.enabled
    }

    /// Explicitly disabled module names.
    pub fn disabled(&self) -> &BTreeSet<String> {
        &self.disabled
    }

    /// Whether the manifest declares an `enabled` allow-list.
    pub fn is_allow_list(&self) -> bool {
        self.allow_list
    }

    /// Resolve whether `name` is enabled.
    ///
    /// `disabled` always wins; an active allow-list admits only its members;
    /// otherwise the module is enabled.
    pub fn is_enabled(&self, name: &str) -> bool {
        if self.disabled.contains(name) {
            return false;
        }
        !self.allow_list || self.enabled.contains(name)
    }

    /// Enable `name` (idempotent).
    ///
    /// Removing the name from `disabled` is enough in default-on mode; the
    /// allow-list is only extended when it is already active, so enabling one
    /// module never disables the rest.
    pub fn enable(&mut self, name: &str) {
        self.disabled.remove(name);
        if self.allow_list {
            self.enabled.insert(name.to_string());
        }
    }

    /// Disable `name` (idempotent).
    ///
    /// The allow-list stays active when it was active, so disabling the last
    /// allowed module yields `enabled = []` rather than reverting to
    /// default-on.
    pub fn disable(&mut self, name: &str) {
        if self.allow_list {
            self.enabled.remove(name);
        }
        self.disabled.insert(name.to_string());
    }

    /// Write the state back, preserving the rest of the manifest file.
    ///
    /// Creates the parent directory (and the file) when missing. Only the
    /// `[modules]` `enabled`/`disabled` keys are touched: `enabled` is written
    /// only when an allow-list is active (including the empty `enabled = []`
    /// form), and an empty deny-list prunes its key.
    pub fn save(&self) -> ModuleResult<()> {
        let existing = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
            Err(source) => {
                return Err(ModuleError::Write {
                    path: self.path.clone(),
                    source,
                })
            }
        };
        let mut document = if existing.trim().is_empty() {
            DocumentMut::new()
        } else {
            existing
                .parse::<DocumentMut>()
                .map_err(|error| ModuleError::Manifest {
                    path: self.path.clone(),
                    message: error.to_string(),
                })?
        };

        {
            let root = document.as_table_mut();
            if !root.contains_key("modules") {
                root.insert("modules", Item::Table(Table::new()));
            }
            let modules = root
                .get_mut("modules")
                .and_then(|item| item.as_table_mut())
                .ok_or_else(|| ModuleError::Manifest {
                    path: self.path.clone(),
                    message: "`modules` must be a table".to_string(),
                })?;
            if self.allow_list {
                // An allow-list that disables everything must round-trip as
                // `enabled = []`, so the empty array is written, not pruned.
                write_names(modules, "enabled", &self.enabled);
            } else {
                modules.remove("enabled");
            }
            if self.disabled.is_empty() {
                modules.remove("disabled");
            } else {
                write_names(modules, "disabled", &self.disabled);
            }
        }

        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|source| ModuleError::Write {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
        }
        std::fs::write(&self.path, document.to_string()).map_err(|source| ModuleError::Write {
            path: self.path.clone(),
            source,
        })
    }

    /// Build state from a parsed `[modules]` table.
    fn from_table(path: PathBuf, table: ModulesTable) -> Self {
        Self {
            path,
            allow_list: table.enabled.is_some(),
            enabled: table.enabled.unwrap_or_default().into_iter().collect(),
            disabled: table.disabled.into_iter().collect(),
        }
    }
}

/// Discover the module crates under `<root>/modules`, applying `manifest`.
///
/// A directory qualifies as a module when it contains a `Cargo.toml`. Results
/// are sorted by name so callers never depend on filesystem enumeration order.
/// A missing `modules/` directory yields an empty list.
pub fn discover_modules(
    root: &Path,
    manifest: &ModuleManifest,
) -> ModuleResult<Vec<ModuleDescriptor>> {
    let dir = root.join(MODULES_DIR);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(ModuleError::Discovery { path: dir, source }),
    };

    let mut descriptors = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ModuleError::Discovery {
            path: dir.clone(),
            source,
        })?;
        let path = entry.path();
        if !path.is_dir() || !path.join("Cargo.toml").is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let name = name.to_string();
        let version = read_module_version(&path, &name)?;
        descriptors.push(ModuleDescriptor {
            enabled: manifest.is_enabled(&name),
            path: PathBuf::from(MODULES_DIR).join(&name),
            version,
            name,
        });
    }
    descriptors.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(descriptors)
}

/// Read the `[module] version` declared by `modules/<name>/config/<name>.toml`.
fn read_module_version(dir: &Path, name: &str) -> ModuleResult<Option<String>> {
    let config = dir.join("config").join(format!("{name}.toml"));
    if !config.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&config).map_err(|source| ModuleError::Discovery {
        path: config.clone(),
        source,
    })?;
    let document: ModuleConfigDocument =
        toml::from_str(&text).map_err(|error| ModuleError::Manifest {
            path: config,
            message: error.to_string(),
        })?;
    Ok(document.module.and_then(|metadata| metadata.version))
}

/// Parse a manifest file into the deserialization shim.
///
/// A missing file is an empty manifest: the caller may then add state and
/// [`ModuleManifest::save`] will create it.
fn read_document(path: &Path) -> ModuleResult<ManifestDocument> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(ManifestDocument::default()),
        Err(source) => {
            return Err(ModuleError::Discovery {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    toml::from_str(&text).map_err(|error| ModuleError::Manifest {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

/// Write one `[modules]` name list as a TOML array.
fn write_names(table: &mut Table, key: &str, names: &BTreeSet<String>) {
    let mut array = Array::new();
    for name in names {
        array.push(name.as_str());
    }
    table.insert(key, Item::Value(Value::Array(array)));
}
