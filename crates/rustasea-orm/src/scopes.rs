//! Named query scopes and global query scopes.
//!
//! A **named** scope ([`ScopeRegistry`]) is a closure applied on demand — the
//! Eloquent pattern (`scopeActive`, `scopeVerified`) adapted to Rust. Registries
//! are keyed `table.name` so multiple models can declare identically-named
//! scopes.
//!
//! A **global** scope ([`GlobalScope`]) is applied automatically to every model
//! query (see [`crate::model::Model::query`]) unless bypassed. The built-in
//! [`SoftDeletesScope`] hides soft-deleted rows by default, mirroring Eloquent's
//! `SoftDeletes` global scope. Scopes are registered per table and can be
//! bypassed wholesale ([`crate::QueryBuilder::without_global_scopes`]) or by
//! type ([`crate::QueryBuilder::without_global_scope`]).

use crate::builder::QueryBuilder;
use crate::error::{OrmError, Result};
use std::any::TypeId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

/// Named scope that can be applied to a builder.
pub type Scope = Box<dyn Fn(&mut QueryBuilder) + Send + Sync>;

/// Registry of named scopes per model table.
#[derive(Default)]
pub struct ScopeRegistry {
    scopes: HashMap<String, Vec<Scope>>,
}

impl ScopeRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a named scope for a table.
    pub fn register<F>(&mut self, table: &str, name: &str, scope: F)
    where
        F: Fn(&mut QueryBuilder) + Send + Sync + 'static,
    {
        self.scopes
            .entry(format!("{table}.{name}"))
            .or_default()
            .push(Box::new(scope));
    }

    /// Apply every scope registered under `table.name` to the builder.
    pub fn apply(&self, builder: &mut QueryBuilder, table: &str, name: &str) {
        if let Some(scopes) = self.scopes.get(&format!("{table}.{name}")) {
            for scope in scopes {
                scope(builder);
            }
        }
    }
}

/// A global query scope applied automatically to every model query.
///
/// Implementors mutate the builder (typically adding a `WHERE` guard). The
/// [`id`](GlobalScope::id) is a stable identifier used to detect duplicate
/// registrations; bypass a scope per query with
/// [`crate::QueryBuilder::without_global_scope::<S>`], which matches on the
/// concrete type `S`.
pub trait GlobalScope: Send + Sync + 'static {
    /// Stable identifier, unique per table (e.g. `"soft_deletes"`).
    fn id(&self) -> &'static str;

    /// Apply the scope's constraints to `builder`.
    fn apply(&self, builder: &mut QueryBuilder);
}

/// The built-in soft-delete global scope — appends `deleted_at IS NULL`.
///
/// Registered by [`crate::model::Model::global_scopes`] whenever the model uses
/// soft deletes, so deleted rows are hidden from every model query by default.
#[derive(Debug, Default, Clone, Copy)]
pub struct SoftDeletesScope;

impl GlobalScope for SoftDeletesScope {
    /// Stable identifier for the soft-delete scope.
    fn id(&self) -> &'static str {
        "soft_deletes"
    }

    /// Hide soft-deleted rows (`deleted_at IS NULL`), idempotently.
    fn apply(&self, builder: &mut QueryBuilder) {
        builder.apply_soft_delete_guard();
    }
}

/// A global scope backed by a closure — the ergonomic constructor for ad-hoc
/// global scopes (`GlobalScope` for a `move |builder| { … }`).
pub struct CallbackScope<F> {
    id: &'static str,
    apply: F,
}

impl<F> CallbackScope<F>
where
    F: Fn(&mut QueryBuilder) + Send + Sync + 'static,
{
    /// Build a global scope with a stable `id` and an `apply` closure.
    pub fn new(id: &'static str, apply: F) -> Self {
        Self { id, apply }
    }
}

impl<F> GlobalScope for CallbackScope<F>
where
    F: Fn(&mut QueryBuilder) + Send + Sync + 'static,
{
    /// The caller-supplied identifier.
    fn id(&self) -> &'static str {
        self.id
    }

    /// Invoke the wrapped closure.
    fn apply(&self, builder: &mut QueryBuilder) {
        (self.apply)(builder);
    }
}

/// A registered global scope together with its concrete type identity.
///
/// The [`TypeId`] captured at construction lets the builder dedupe and bypass
/// scopes by type ([`crate::QueryBuilder::without_global_scope::<S>`]) without
/// requiring `GlobalScope` to expose a downcast hook.
#[derive(Clone)]
pub struct GlobalScopeEntry {
    type_id: TypeId,
    id: &'static str,
    scope: Arc<dyn GlobalScope>,
}

impl GlobalScopeEntry {
    /// Wrap a concrete global scope, capturing its [`TypeId`].
    pub fn new<S: GlobalScope + 'static>(scope: S) -> Self {
        Self {
            type_id: TypeId::of::<S>(),
            id: scope.id(),
            scope: Arc::new(scope),
        }
    }

    /// The scope's stable identifier.
    pub fn id(&self) -> &'static str {
        self.id
    }

    /// The concrete type of the wrapped scope.
    pub fn type_id(&self) -> TypeId {
        self.type_id
    }

    /// Apply the wrapped scope to `builder`.
    pub fn apply(&self, builder: &mut QueryBuilder) {
        self.scope.apply(builder);
    }
}

/// Registry of global scopes, keyed by model table.
///
/// [`register`](GlobalScopeRegistry::register) rejects an empty table name and a
/// duplicate scope id for the same table with [`OrmError::Scope`]. The
/// process-wide instance is reachable via [`register_global_scope`] /
/// [`registered_global_scopes`].
#[derive(Default)]
pub struct GlobalScopeRegistry {
    scopes: HashMap<String, Vec<GlobalScopeEntry>>,
}

impl GlobalScopeRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a global scope for `table`.
    ///
    /// Returns [`OrmError::Scope`] for an empty table name or a scope whose
    /// [`id`](GlobalScope::id) is already registered for that table.
    pub fn register<S: GlobalScope + 'static>(&mut self, table: &str, scope: S) -> Result<()> {
        if table.trim().is_empty() {
            return Err(OrmError::Scope("empty table name".into()));
        }
        let entry = GlobalScopeEntry::new(scope);
        let bucket = self.scopes.entry(table.to_string()).or_default();
        if bucket.iter().any(|existing| existing.id() == entry.id()) {
            return Err(OrmError::Scope(format!(
                "duplicate global scope id `{}` for table `{table}`",
                entry.id()
            )));
        }
        bucket.push(entry);
        Ok(())
    }

    /// The global scopes registered for `table` (empty when none).
    pub fn for_table(&self, table: &str) -> &[GlobalScopeEntry] {
        self.scopes.get(table).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Number of registered scopes across every table.
    pub fn len(&self) -> usize {
        self.scopes.values().map(Vec::len).sum()
    }

    /// Whether no scopes are registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Process-wide global-scope registry populated at application boot.
///
/// Mirrors the migration registry: [`register_global_scope`] writes into it and
/// [`registered_global_scopes`] reads it back when a model query is built.
static GLOBAL_SCOPES: OnceLock<Mutex<GlobalScopeRegistry>> = OnceLock::new();

/// The process-wide global-scope registry, initialised on first use.
fn global_scope_registry() -> &'static Mutex<GlobalScopeRegistry> {
    GLOBAL_SCOPES.get_or_init(|| Mutex::new(GlobalScopeRegistry::new()))
}

/// Lock the global-scope registry, surfacing poison as [`OrmError::Scope`].
fn lock_scopes(
    registry: &Mutex<GlobalScopeRegistry>,
) -> Result<MutexGuard<'_, GlobalScopeRegistry>> {
    registry
        .lock()
        .map_err(|_| OrmError::Scope("global scope registry poisoned".into()))
}

/// Register a global scope for `table` in the process-wide registry.
///
/// Call once per scope from application boot code (before serving requests).
/// Rejects an empty table name, a duplicate scope id, and a poisoned registry
/// with [`OrmError::Scope`].
pub fn register_global_scope<S: GlobalScope + 'static>(table: &str, scope: S) -> Result<()> {
    lock_scopes(global_scope_registry())?.register(table, scope)
}

/// Read the registered global scopes for `table`, surfacing poison.
///
/// Use this when a poisoned registry must abort the query; the infallible
/// [`registered_global_scopes`] recovers a poisoned lock for the model path.
pub fn try_registered_global_scopes(table: &str) -> Result<Vec<GlobalScopeEntry>> {
    let guard = lock_scopes(global_scope_registry())?;
    Ok(guard.for_table(table).to_vec())
}

/// Read the registered global scopes for `table`, recovering a poisoned lock.
///
/// A partially-registered snapshot beats silently dropping scopes; call
/// [`try_registered_global_scopes`] when poison must be observable.
pub fn registered_global_scopes(table: &str) -> Vec<GlobalScopeEntry> {
    match lock_scopes(global_scope_registry()) {
        Ok(guard) => guard.for_table(table).to_vec(),
        Err(_poisoned) => {
            let guard = global_scope_registry()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.for_table(table).to_vec()
        }
    }
}

/// Clear every registered global scope — a bootstrap/test reset hook.
///
/// Recovers a poisoned mutex instead of silently skipping the reset, so a
/// panic in another thread cannot leave the process-wide registry in a
/// permanently dirty state. The poison flag is cleared too, so later
/// registrations and reads succeed rather than surfacing [`OrmError::Scope`].
pub fn reset_global_scopes() {
    let registry = global_scope_registry();
    let mut guard = registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = GlobalScopeRegistry::new();
    // Clear the poison flag so later lock attempts succeed rather than
    // surfacing `OrmError::Scope`.
    drop(guard);
    registry.clear_poison();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Value;

    /// Verifies scopes compose additively on a single builder.
    #[test]
    fn scopes_compose() {
        let mut registry = ScopeRegistry::new();
        registry.register("users", "active", |b| {
            *b = std::mem::take(b).where_null("deleted_at");
        });
        registry.register("users", "verified", |b| {
            *b = std::mem::take(b).where_null("email_verified_at");
        });
        let qb = QueryBuilder::table("users")
            .with_scope(&registry, "active")
            .with_scope(&registry, "verified");
        assert_eq!(
            qb.to_sql().unwrap(),
            "SELECT * FROM users WHERE deleted_at IS NULL AND email_verified_at IS NULL"
        );
    }

    /// Verifies the soft-delete scope appends `deleted_at IS NULL`.
    #[test]
    fn soft_deletes_scope_hides_trashed() {
        let qb = QueryBuilder::table("users")
            .with_global_scopes(vec![GlobalScopeEntry::new(SoftDeletesScope)]);
        assert_eq!(
            qb.to_sql().unwrap(),
            "SELECT * FROM users WHERE deleted_at IS NULL"
        );
        assert!(qb.has_global_scope::<SoftDeletesScope>());
    }

    /// Verifies `without_global_scopes` drops every scope condition.
    #[test]
    fn without_global_scopes_bypasses_all() {
        let qb = QueryBuilder::table("users")
            .with_global_scopes(vec![GlobalScopeEntry::new(SoftDeletesScope)])
            .without_global_scopes();
        assert_eq!(qb.to_sql().unwrap(), "SELECT * FROM users");
        assert!(qb.global_scopes_bypassed());
        assert!(qb.global_scopes().is_empty());
    }

    /// Verifies `without_global_scope::<S>` drops only the named scope.
    #[test]
    fn without_global_scope_bypasses_one() {
        let tenant = CallbackScope::new("tenant", |b| {
            *b = std::mem::take(b).where_eq("tenant_id", Value::Int(7));
        });
        let qb = QueryBuilder::table("users")
            .with_global_scopes(vec![
                GlobalScopeEntry::new(SoftDeletesScope),
                GlobalScopeEntry::new(tenant),
            ])
            .without_global_scope::<SoftDeletesScope>();
        let sql = qb.to_sql().unwrap();
        assert!(!sql.contains("deleted_at"), "{sql}");
        assert!(sql.contains("tenant_id = $1"), "{sql}");
    }

    /// Verifies registration rejects an empty table name.
    #[test]
    fn register_rejects_empty_table() {
        let mut registry = GlobalScopeRegistry::new();
        let error = registry.register("  ", SoftDeletesScope).unwrap_err();
        assert!(matches!(error, OrmError::Scope(_)), "got {error:?}");
    }

    /// Verifies registration rejects a duplicate scope id for one table.
    #[test]
    fn register_rejects_duplicate_id() {
        let mut registry = GlobalScopeRegistry::new();
        registry.register("users", SoftDeletesScope).unwrap();
        let error = registry.register("users", SoftDeletesScope).unwrap_err();
        assert!(matches!(error, OrmError::Scope(_)), "got {error:?}");
        // A different table is unaffected by the duplicate guard.
        assert!(registry.register("posts", SoftDeletesScope).is_ok());
    }

    /// Verifies a poisoned registry surfaces the typed scope error.
    #[test]
    fn poisoned_registry_surfaces_typed_error() {
        let registry = Arc::new(Mutex::new(GlobalScopeRegistry::new()));
        let poisoned = Arc::clone(&registry);
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.lock().unwrap();
            panic!("poison the global scope registry");
        })
        .join();

        let error = match lock_scopes(&registry) {
            Ok(_) => panic!("poisoned registry must surface an error"),
            Err(error) => error,
        };
        assert!(matches!(error, OrmError::Scope(_)), "got {error:?}");
    }

    /// Verifies `reset_global_scopes` recovers a poisoned process-wide registry
    /// so later registration succeeds instead of silently no-op'ing.
    #[test]
    fn reset_recovers_poisoned_registry() {
        // Poison the shared registry from a worker thread that panics while
        // holding the lock.
        let poisoned = std::thread::spawn(|| {
            let _guard = global_scope_registry().lock().unwrap();
            panic!("poison the global scope registry");
        });
        let _ = poisoned.join();

        // Precondition: the registry is genuinely poisoned.
        assert!(lock_scopes(global_scope_registry()).is_err());

        // The reset must recover and clear, not silently do nothing.
        reset_global_scopes();

        // A subsequent registration/read now succeeds.
        register_global_scope("reset_probe", SoftDeletesScope).unwrap();
        assert_eq!(registered_global_scopes("reset_probe").len(), 1);

        // Leave the process-wide registry clean for other tests.
        reset_global_scopes();
    }
}
