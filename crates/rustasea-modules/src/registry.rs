//! Deterministic module registry.
//!
//! [`ModuleRegistry`] holds every known [`Module`] keyed by name in a
//! [`BTreeMap`], so registration, route mounting, provider boot, and migration
//! collection always run in the same alphabetical order regardless of the
//! order modules were added or how the filesystem enumerated them. A module
//! that the manifest disables contributes nothing: no routes, no providers,
//! no migrations.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustasea_foundation::{Application, ServiceProvider};
use rustasea_orm::Migrator;
use rustasea_router::Router;

use crate::error::{ModuleError, ModuleResult};
use crate::manifest::ModuleManifest;
use crate::module::{BoxedMigration, Module};

/// A registered module plus the on-disk metadata and enabled state.
#[derive(Clone)]
pub struct RegisteredModule {
    module: Arc<dyn Module>,
    path: Option<PathBuf>,
    enabled: bool,
}

impl RegisteredModule {
    /// The module's stable name (the registry key).
    pub fn name(&self) -> &str {
        self.module.name()
    }

    /// The module's declared version.
    pub fn version(&self) -> &str {
        self.module.version()
    }

    /// Workspace-relative module directory, when the module was discovered
    /// from disk rather than constructed in code.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Whether the manifest currently enables the module.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Borrow the underlying module.
    pub fn module(&self) -> &dyn Module {
        self.module.as_ref()
    }
}

/// Ordered registry of application modules.
///
/// Ordering is alphabetical by module name and is stable across processes, so
/// route tables and migration batches are reproducible.
#[derive(Default, Clone)]
pub struct ModuleRegistry {
    modules: BTreeMap<String, RegisteredModule>,
}

impl ModuleRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a module constructed in code (no on-disk path).
    ///
    /// Modules are enabled by default; call [`ModuleRegistry::apply_manifest`]
    /// to apply persisted enable/disable state.
    pub fn register(&mut self, module: Arc<dyn Module>) -> ModuleResult<()> {
        self.register_at(module, None)
    }

    /// Register a module discovered from `path` (workspace-relative).
    pub fn register_at(
        &mut self,
        module: Arc<dyn Module>,
        path: Option<PathBuf>,
    ) -> ModuleResult<()> {
        let name = module.name().to_string();
        validate_name(&name)?;
        if self.modules.contains_key(&name) {
            return Err(ModuleError::Duplicate { name });
        }
        self.modules.insert(
            name,
            RegisteredModule {
                module,
                path,
                enabled: true,
            },
        );
        Ok(())
    }

    /// Apply persisted enable/disable state to every registered module.
    pub fn apply_manifest(&mut self, manifest: &ModuleManifest) {
        for (name, registered) in &mut self.modules {
            registered.enabled = manifest.is_enabled(name);
        }
    }

    /// Enable a registered module.
    pub fn enable(&mut self, name: &str) -> ModuleResult<()> {
        self.set_enabled(name, true)
    }

    /// Disable a registered module.
    pub fn disable(&mut self, name: &str) -> ModuleResult<()> {
        self.set_enabled(name, false)
    }

    /// Look up a registered module by name.
    pub fn get(&self, name: &str) -> Option<&RegisteredModule> {
        self.modules.get(name)
    }

    /// Whether a module with `name` is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.modules.contains_key(name)
    }

    /// Every registered module, in deterministic order.
    pub fn modules(&self) -> impl Iterator<Item = &RegisteredModule> {
        self.modules.values()
    }

    /// Names of every registered module, in deterministic order.
    pub fn names(&self) -> Vec<String> {
        self.modules.keys().cloned().collect()
    }

    /// Names of enabled modules, in deterministic order.
    pub fn enabled_names(&self) -> Vec<String> {
        self.enabled()
            .map(|module| module.name().to_string())
            .collect()
    }

    /// Names of disabled modules, in deterministic order.
    pub fn disabled_names(&self) -> Vec<String> {
        self.modules
            .values()
            .filter(|module| !module.enabled)
            .map(|module| module.name().to_string())
            .collect()
    }

    /// Whether the registry holds no modules.
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    /// Number of registered modules.
    pub fn len(&self) -> usize {
        self.modules.len()
    }

    /// Mount every enabled module's routes onto `router`, in order.
    pub fn routes(&self, router: &mut Router) {
        for registered in self.enabled() {
            registered.module.routes(router);
        }
    }

    /// Collect every enabled module's providers, in order.
    ///
    /// The returned boxes are the same values handed to
    /// [`ModuleRegistry::install`]; call once per lifecycle phase so a
    /// stateful provider observes its own `register` then `boot`.
    pub fn providers(&self) -> Vec<Box<dyn ServiceProvider>> {
        self.enabled()
            .flat_map(|registered| registered.module.providers())
            .collect()
    }

    /// Add every enabled module's providers to `app`'s boot DAG, in order.
    pub fn install(&self, app: &mut Application) {
        for provider in self.providers() {
            app.provider(provider);
        }
    }

    /// Build a [`Migrator`] from every enabled module's migrations, in order.
    pub fn migrator(&self) -> Migrator {
        let mut migrator = Migrator::new();
        for registered in self.enabled() {
            for migration in registered.module.migrations() {
                migrator.add(BoxedMigration::new(migration));
            }
        }
        migrator
    }

    /// Enabled modules, in deterministic order.
    fn enabled(&self) -> impl Iterator<Item = &RegisteredModule> {
        self.modules.values().filter(|module| module.enabled)
    }

    /// Mutate one module's enabled flag, erroring on an unknown name.
    fn set_enabled(&mut self, name: &str, enabled: bool) -> ModuleResult<()> {
        match self.modules.get_mut(name) {
            Some(registered) => {
                registered.enabled = enabled;
                Ok(())
            }
            None => Err(ModuleError::Unknown {
                name: name.to_string(),
            }),
        }
    }
}

/// Validate a module name as a workspace-safe directory/registry key.
///
/// Names are lowercase ASCII with digits and underscores — the same shape as
/// the `modules/<name>` directory and the Cargo package `module-<name>`.
pub fn validate_name(name: &str) -> ModuleResult<()> {
    let invalid = |reason: &str| {
        Err(ModuleError::InvalidName {
            name: name.to_string(),
            reason: reason.to_string(),
        })
    };
    if name.is_empty() {
        return invalid("name must not be empty");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return invalid("name may only contain a-z, 0-9, and _ (snake_case)");
    }
    Ok(())
}
