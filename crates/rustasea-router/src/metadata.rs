//! Named-middleware registry and typed router build errors.
//!
//! Named middleware is registered as a black-box transform over an axum
//! [`MethodRouter`], or as a parameterized factory for Laravel-style
//! `name:param` specs. At build time each route's declared middleware
//! identifiers are resolved through [`MiddlewareRegistry`] and applied to the
//! route's method router, so a route that names an unregistered middleware
//! fails the build with [`RouteError::UnknownMiddleware`] instead of silently
//! dropping the layer.
//!
//! This module also hosts the registration hooks that consume the
//! `__RUSTASEA_MIDDLEWARE_<Fn>` metadata emitted by the `#[middleware]`
//! attribute: [`Router::middleware_meta`] and
//! [`Router::register_middleware_factory`].

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

/// Factory building a middleware transform from a `name:param` suffix.
///
/// Registered by [`crate::Router::register_middleware_factory`] and invoked by
/// [`MiddlewareRegistry::resolve`] with the text after the first `:` in a
/// route's middleware spec (e.g. `"60,1"` for `"throttle:60,1"`). Returning
/// `None` rejects the parameters, which fails the build closed with
/// [`RouteError::UnknownMiddleware`].
pub type MiddlewareFactory = Arc<dyn Fn(&str) -> Option<MiddlewareApply> + Send + Sync>;

/// A registered middleware: a fixed transform or a parameterized factory.
#[derive(Clone)]
enum Registration {
    /// Applied verbatim, ignoring any `name:param` suffix.
    Plain(MiddlewareApply),
    /// Built from the `name:param` suffix; `None` rejects the parameters.
    Factory(MiddlewareFactory),
}

/// Registry mapping middleware identifiers to their transform.
///
/// Populated by [`crate::Router::register_middleware`] /
/// [`crate::Router::register_middleware_factory`] and consulted once per route
/// during [`crate::Router::try_into_axum_router`]. An identifier declared by a
/// route but absent here is a build error, not a silent no-op.
#[derive(Default, Clone)]
pub struct MiddlewareRegistry {
    entries: HashMap<String, Registration>,
}

impl MiddlewareRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert (or replace) the fixed transform registered under `name`.
    pub fn insert(&mut self, name: impl Into<String>, apply: MiddlewareApply) {
        self.entries.insert(name.into(), Registration::Plain(apply));
    }

    /// Insert (or replace) the parameterized factory registered under `name`.
    pub fn insert_factory<F>(&mut self, name: impl Into<String>, factory: F)
    where
        F: Fn(&str) -> Option<MiddlewareApply> + Send + Sync + 'static,
    {
        self.entries
            .insert(name.into(), Registration::Factory(Arc::new(factory)));
    }

    /// Look up the fixed transform registered under `name`.
    ///
    /// Returns `None` for an unknown name or a parameterized factory — use
    /// [`MiddlewareRegistry::resolve`] to resolve a full route spec.
    pub fn get(&self, name: &str) -> Option<&MiddlewareApply> {
        match self.entries.get(name) {
            Some(Registration::Plain(apply)) => Some(apply),
            _ => None,
        }
    }

    /// Resolve a route middleware spec (`name` or `name:params`) to a transform.
    ///
    /// An exact-spec match wins first (backward compatible with identifiers
    /// that themselves contain `:`); otherwise the spec splits at the first `:`
    /// into a base name and a parameter suffix. A base name registered as a
    /// factory receives the suffix, while a plain transform ignores it.
    ///
    /// # Errors
    ///
    /// Returns [`RouteError::UnknownMiddleware`] when the spec matches no
    /// registration or when a factory rejects its parameters — never a silent
    /// no-op.
    pub fn resolve(&self, spec: &str) -> Result<MiddlewareApply, RouteError> {
        if let Some(entry) = self.entries.get(spec) {
            return match entry {
                Registration::Plain(apply) => Ok(Arc::clone(apply)),
                Registration::Factory(factory) => {
                    factory("").ok_or_else(|| RouteError::UnknownMiddleware {
                        name: spec.to_string(),
                    })
                }
            };
        }
        let Some((name, params)) = spec.split_once(':') else {
            return Err(RouteError::UnknownMiddleware {
                name: spec.to_string(),
            });
        };
        match self.entries.get(name) {
            Some(Registration::Factory(factory)) => {
                factory(params).ok_or_else(|| RouteError::UnknownMiddleware {
                    name: spec.to_string(),
                })
            }
            Some(Registration::Plain(apply)) => Ok(Arc::clone(apply)),
            None => Err(RouteError::UnknownMiddleware {
                name: spec.to_string(),
            }),
        }
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

    /// Register a parameterized middleware factory for `name`.
    ///
    /// A route spec `name:params` (e.g. `"throttle:60,1"`) resolves to this
    /// factory invoked with `params`. Returning `None` rejects the parameters,
    /// which fails the build closed with [`RouteError::UnknownMiddleware`]
    /// rather than silently applying no middleware.
    pub fn register_middleware_factory<F>(
        &mut self,
        name: impl Into<String>,
        factory: F,
    ) -> &mut Self
    where
        F: Fn(&str) -> Option<MiddlewareApply> + Send + Sync + 'static,
    {
        self.middleware_registry.insert_factory(name, factory);
        self
    }

    /// Declare middleware from `#[middleware]` attribute metadata.
    ///
    /// Consumes the doc-hidden `__RUSTASEA_MIDDLEWARE_<Fn>` spec list emitted
    /// by the `#[middleware]` attribute so a controller handler's declared
    /// middleware is enforced at build time. Specs may be parameterized
    /// (`"throttle:60,1"`) and are resolved through
    /// [`MiddlewareRegistry::resolve`].
    ///
    /// Like [`Router::middleware`], the declaration is sticky: it applies to
    /// every subsequent route on the table. Declare it inside a
    /// [`Router::group`] or immediately before the target route so the gate
    /// stays scoped to the handler that declared it.
    pub fn middleware_meta(&mut self, middleware: &[&str]) -> &mut Self {
        self.pending_middleware
            .extend(middleware.iter().map(|spec| (*spec).to_string()));
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

    /// `resolve` accepts a plain name and ignores a `name:param` suffix.
    #[test]
    fn resolve_plain_name_and_suffix() {
        let mut registry = MiddlewareRegistry::new();
        registry.insert("auth", Arc::new(|mr| mr));
        assert!(registry.resolve("auth").is_ok());
        assert!(registry.resolve("auth:jwt").is_ok());
    }

    /// `resolve` invokes a factory with the text after the first colon.
    #[test]
    fn resolve_factory_receives_parameters() {
        let mut registry = MiddlewareRegistry::new();
        registry.insert_factory("throttle", |params| {
            assert_eq!(params, "60,1");
            Some(Arc::new(|mr| mr))
        });
        assert!(registry.resolve("throttle:60,1").is_ok());
    }

    /// An unregistered spec and a factory that rejects its parameters both
    /// resolve to the typed `UnknownMiddleware` error.
    #[test]
    fn resolve_unknown_specs_are_typed_errors() {
        let mut registry = MiddlewareRegistry::new();
        registry.insert_factory("throttle", |_| None);
        assert_eq!(
            registry.resolve("ghost").err(),
            Some(RouteError::UnknownMiddleware {
                name: "ghost".to_string()
            })
        );
        assert_eq!(
            registry.resolve("throttle:bad").err(),
            Some(RouteError::UnknownMiddleware {
                name: "throttle:bad".to_string()
            })
        );
    }
}
