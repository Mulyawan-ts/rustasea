//! Named-middleware registry and typed router build errors.
//!
//! Named middleware is registered as a black-box transform over an axum
//! [`MethodRouter`]. At build time each route's declared middleware identifiers
//! are resolved through [`MiddlewareRegistry`] and applied to the route's
//! method router, so a route that names an unregistered middleware fails the
//! build with [`RouteError::UnknownMiddleware`] instead of silently dropping
//! the layer.

use std::collections::HashMap;
use std::sync::Arc;

use axum::routing::MethodRouter;

use crate::router::Router;

/// Black-box middleware transform applied to a route's method router.
///
/// The closure receives the route's [`MethodRouter`] and returns the wrapped
/// router; typical implementations call `MethodRouter::layer` with a tower
/// layer (e.g. `axum::middleware::from_fn`). Stored behind an [`Arc`] so one
/// registration can be applied to many routes without cloning the layer.
pub type MiddlewareApply = Arc<dyn Fn(MethodRouter<()>) -> MethodRouter<()> + Send + Sync>;

/// Registry mapping middleware identifiers to their transform.
///
/// Populated by [`crate::Router::register_middleware`] and consulted once per
/// route during [`crate::Router::try_into_axum_router`]. An identifier declared
/// by a route but absent here is a build error, not a silent no-op.
#[derive(Default, Clone)]
pub struct MiddlewareRegistry {
    entries: HashMap<String, MiddlewareApply>,
}

impl MiddlewareRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert (or replace) the transform registered under `name`.
    pub fn insert(&mut self, name: impl Into<String>, apply: MiddlewareApply) {
        self.entries.insert(name.into(), apply);
    }

    /// Look up the transform registered under `name`.
    pub fn get(&self, name: &str) -> Option<&MiddlewareApply> {
        self.entries.get(name)
    }

    /// Whether the registry holds no middleware.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Absorb every entry from `other`, replacing same-named transforms.
    ///
    /// Used by [`crate::Router::group`] so middleware registered inside a group
    /// is available when the parent router is compiled.
    pub fn merge(&mut self, other: MiddlewareRegistry) {
        self.entries.extend(other.entries);
    }
}

/// Typed error returned by [`crate::Router::try_into_axum_router`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    /// A route declared a middleware identifier that was never registered.
    UnknownMiddleware {
        /// The unregistered middleware identifier.
        name: String,
    },
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownMiddleware { name } => {
                write!(formatter, "unknown middleware `{name}`")
            }
        }
    }
}

impl std::error::Error for RouteError {}

impl Router {
    /// Register a named middleware transform for use by route declarations.
    ///
    /// The `apply` closure wraps a route's [`MethodRouter`] — typically with
    /// `MethodRouter::layer` — and runs for every route (plain, resource,
    /// action-bound, or redirect) that declares this identifier via
    /// [`Router::middleware`]. Middleware may be registered before or after the
    /// routes that reference it; resolution happens at build time in
    /// [`Router::try_into_axum_router`].
    pub fn register_middleware<F>(&mut self, name: impl Into<String>, apply: F) -> &mut Self
    where
        F: Fn(MethodRouter<()>) -> MethodRouter<()> + Send + Sync + 'static,
    {
        self.middleware_registry.insert(name, Arc::new(apply));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inserted middleware is retrievable; missing identifiers return `None`.
    #[test]
    fn registry_insert_and_get_roundtrip() {
        let mut registry = MiddlewareRegistry::new();
        assert!(registry.is_empty());
        registry.insert("auth", Arc::new(|mr| mr));
        assert!(!registry.is_empty());
        assert!(registry.get("auth").is_some());
        assert!(registry.get("missing").is_none());
    }

    /// Unknown middleware renders a human-readable diagnostic.
    #[test]
    fn unknown_middleware_display() {
        let error = RouteError::UnknownMiddleware {
            name: "auth".to_string(),
        };
        assert_eq!(error.to_string(), "unknown middleware `auth`");
    }
}
