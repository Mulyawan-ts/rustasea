//! RustaSea modular application support (ADOPT-027).
//!
//! Splits a large application into self-contained module crates, mirroring
//! `nwidart/laravel-modules`: each `modules/<name>` crate implements the
//! [`Module`] contract and contributes routes, service providers, and
//! migrations. [`ModuleRegistry`] mounts everything in a deterministic
//! (alphabetical) order, and [`ModuleManifest`] persists which modules are
//! enabled so a disabled module contributes nothing at boot.
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use rustasea_modules::{ModuleManifest, ModuleRegistry};
//!
//! # fn main() -> Result<(), rustasea_modules::ModuleError> {
//! let root = std::path::Path::new(".");
//! let manifest = ModuleManifest::read(root)?;
//! let mut registry = ModuleRegistry::new();
//! registry.apply_manifest(&manifest);
//! # let _ = Arc::new(());
//! # Ok(())
//! # }
//! ```
//!
//! The CLI counterpart (`make:module`, `module:list`, `module:enable`,
//! `module:disable`) scaffolds and toggles these crates from the console.

#![deny(missing_docs)]

pub mod error;
pub mod manifest;
pub mod module;
pub mod registry;

pub use error::{ModuleError, ModuleResult};
pub use manifest::{
    discover_modules, ModuleDescriptor, ModuleManifest, APP_MANIFEST, MODULES_DIR, MODULES_MANIFEST,
};
pub use module::{BoxedMigration, Module};
pub use registry::{validate_name, ModuleRegistry, RegisteredModule};
