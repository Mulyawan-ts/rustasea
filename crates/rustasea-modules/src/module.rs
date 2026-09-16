//! The runtime module contract and its migration adapter.
//!
//! A [`Module`] is one self-contained feature of a modular application: it
//! contributes HTTP routes, service providers, and database migrations, and
//! [`crate::ModuleRegistry`] drives all three in a deterministic order. The
//! trait is object-safe so applications can hold `Arc<dyn Module>` values and
//! the registry can interleave modules shipped as separate workspace crates.

use rustasea_foundation::ServiceProvider;
use rustasea_orm::{Migration, Result as OrmResult};
use rustasea_router::Router;

/// One self-contained application module.
///
/// Implementors declare a stable [`Module::name`] (the registry key and the
/// `modules/<name>` directory name) and override the hooks they need. Every
/// hook defaults to a no-op, so a module that only adds routes implements just
/// [`Module::routes`].
pub trait Module: Send + Sync {
    /// Unique, stable module name (e.g. `blog`).
    fn name(&self) -> &str;

    /// Module version recorded in `module:list`.
    ///
    /// Defaults to the compiling crate's package version.
    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    /// Register the module's HTTP routes.
    fn routes(&self, _router: &mut Router) {}

    /// Service providers registered at boot.
    fn providers(&self) -> Vec<Box<dyn ServiceProvider>> {
        Vec::new()
    }

    /// Database migrations contributed by the module.
    fn migrations(&self) -> Vec<Box<dyn Migration>> {
        Vec::new()
    }
}

/// Adapter letting a boxed [`Migration`] satisfy [`rustasea_orm::Migrator`]'s
/// `M: Migration` registration bound.
///
/// `Box<dyn Migration>` cannot implement `Migration` directly (both the trait
/// and the type are foreign to this crate), so the registry wraps each boxed
/// migration in this local newtype before calling `Migrator::add`.
pub struct BoxedMigration(Box<dyn Migration>);

impl BoxedMigration {
    /// Wrap a boxed migration for registration.
    pub fn new(migration: Box<dyn Migration>) -> Self {
        Self(migration)
    }
}

impl Migration for BoxedMigration {
    /// Delegate the migration name.
    fn name(&self) -> &str {
        self.0.name()
    }

    /// Delegate the forward SQL body.
    fn up(&self) -> OrmResult<String> {
        self.0.up()
    }

    /// Delegate the reverse SQL body.
    fn down(&self) -> OrmResult<String> {
        self.0.down()
    }
}
