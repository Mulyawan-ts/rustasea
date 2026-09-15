//! Record-level `#[authorize]` enforcement — registry and resource contract.
//!
//! The `#[authorize("update", &post)]` attribute emits a
//! `__RUSTASEA_AUTHORIZE_<Fn>: (&str, &str)` metadata const recording the
//! ability name and a resource id. [`Router::authorize_meta`] attaches that
//! declaration to the route being registered; at build time
//! [`crate::Router::try_into_axum_router`] resolves the resource id through
//! [`AuthorizeRegistry`] and wraps the route in an authorization layer that
//! runs *innermost* — after every declared middleware and before the handler —
//! so a denied request is short-circuited with the Gate's `403` envelope.
//!
//! # Why a registry of resources
//!
//! Rust has no runtime reflection, so the ability check cannot discover the
//! target model's type from the attribute alone. Following the explicit
//! registration precedent of [`crate::MiddlewareRegistry`] (LARAVEL-003), the
//! application registers an [`AuthorizeResource`] per resource id at boot
//! ([`Router::register_authorizer`]); the implementation captures the concrete
//! model type and the shared Gate, and pulls the resolved instance out of the
//! request extensions at request time. An unregistered resource id fails the
//! build closed with [`RouteError::UnknownAuthorization`] rather than silently
//! serving the route unprotected.
//!
//! # The resource contract
//!
//! A handler (or a preceding binding middleware) publishes the resolved model
//! by inserting it into the request extensions as a typed value:
//!
//! ```rust,ignore
//! request.extensions_mut().insert(post); // post: Post
//! ```
//!
//! The registered authorizer then retrieves it with
//! `request.extensions().get::<Post>()`, reads the principal an auth middleware
//! stored as `AuthUser`, and calls the Gate. See the `rustasea-auth`
//! `GateResource` adapter for the reference implementation.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::Request;
use axum::response::Response;

use crate::metadata::RouteError;
use crate::route::AuthorizeSpec;
use crate::router::Router;

/// A request-scoped authorizer for one resolved resource type.
///
/// Implemented by the application (or the `rustasea-auth` `GateResource`
/// adapter) and registered with [`Router::register_authorizer`]. The
/// authorization layer calls [`AuthorizeResource::authorize`] once per declared
/// ability; returning `Err(response)` short-circuits the request with that
/// response before the handler runs.
pub trait AuthorizeResource: Send + Sync + 'static {
    /// Enforce `ability` against the request's principal and resolved resource.
    ///
    /// `Ok(())` lets the request proceed; `Err(response)` is returned verbatim
    /// (typically the `403` JSON envelope from the Gate).
    // The `Err` variant is a full `Response`, which axum middleware already
    // passes by value; boxing it would add an allocation on the denial path for
    // no benefit, so the large-`Err` lint is allowed here.
    #[allow(clippy::result_large_err)]
    fn authorize(&self, request: &Request, ability: &str) -> Result<(), Response>;
}

/// Registry mapping `#[authorize]` resource ids to their authorizer.
///
/// Populated by [`Router::register_authorizer`] and consulted during
/// [`crate::Router::try_into_axum_router`]. A resource id declared by a route
/// but absent here is a build error ([`RouteError::UnknownAuthorization`]), not
/// a silent no-op — mirroring [`crate::MiddlewareRegistry`].
///
/// The registry also holds the optional *ability gate* — the authorizer
/// enforcing the ability-only form (`#[authorize("users.edit")]`), registered
/// with [`Router::authorize_abilities`]. Because an ability-only check has no
/// resource id, there is exactly one ability gate per router.
#[derive(Default, Clone)]
pub struct AuthorizeRegistry {
    /// Resource id → shared authorizer.
    resources: HashMap<String, Arc<dyn AuthorizeResource>>,
    /// Optional authorizer enforcing ability-only `#[authorize]` declarations.
    ability: Option<Arc<dyn AuthorizeResource>>,
}

impl AuthorizeRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert (or replace) the authorizer registered under `name`.
    pub fn insert(&mut self, name: impl Into<String>, resource: Arc<dyn AuthorizeResource>) {
        self.resources.insert(name.into(), resource);
    }

    /// Install (or replace) the authorizer enforcing ability-only declarations.
    pub fn set_ability(&mut self, ability: Arc<dyn AuthorizeResource>) {
        self.ability = Some(ability);
    }

    /// The ability gate, if one was registered.
    pub fn ability(&self) -> Option<Arc<dyn AuthorizeResource>> {
        self.ability.clone()
    }

    /// Resolve the authorizer registered under `name`.
    ///
    /// # Errors
    ///
    /// Returns [`RouteError::UnknownAuthorization`] when `name` is unregistered
    /// — the caller treats that as a fail-closed build error.
    pub fn resolve(&self, name: &str) -> Result<Arc<dyn AuthorizeResource>, RouteError> {
        self.resources
            .get(name)
            .cloned()
            .ok_or_else(|| RouteError::UnknownAuthorization {
                name: name.to_string(),
            })
    }

    /// The single registered resource when exactly one is present.
    ///
    /// Used as the fallback for an `#[authorize]` declaration that omitted the
    /// resource id (an empty string): with only one authorizer registered the
    /// target is unambiguous. Returns `None` for zero or many registrations, so
    /// an ambiguous declaration still fails the build closed.
    pub fn sole(&self) -> Option<Arc<dyn AuthorizeResource>> {
        if self.resources.len() == 1 {
            self.resources.values().next().cloned()
        } else {
            None
        }
    }

    /// Whether the registry holds no authorizers.
    pub fn is_empty(&self) -> bool {
        self.resources.is_empty() && self.ability.is_none()
    }

    /// Absorb every entry from `other`, replacing same-named authorizers.
    ///
    /// Used by [`crate::Router::group`] so authorizers registered inside a group
    /// are available when the parent router is compiled. An ability gate in
    /// `other` overrides the current one.
    pub fn merge(&mut self, other: AuthorizeRegistry) {
        self.resources.extend(other.resources);
        if other.ability.is_some() {
            self.ability = other.ability;
        }
    }
}

impl std::fmt::Debug for AuthorizeRegistry {
    /// Manual debug — the boxed authorizers have no `Debug` impl.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut names: Vec<&String> = self.resources.keys().collect();
        names.sort();
        formatter
            .debug_struct("AuthorizeRegistry")
            .field("resources", &names)
            .field("has_ability_gate", &self.ability.is_some())
            .finish()
    }
}

impl Router {
    /// Register the authorizer enforcing an `#[authorize]` resource id.
    ///
    /// The `resource` captures the concrete model type and the shared Gate and
    /// is invoked per request. Registration is keyed by the resource id recorded
    /// in the `#[authorize]` metadata, so a route that declares an unregistered
    /// id fails the build closed with [`RouteError::UnknownAuthorization`]:
    ///
    /// ```rust,ignore
    /// let gate = Arc::new(build_gate());
    /// router.register_authorizer("post", Arc::new(GateResource::<Post>::new(gate)));
    /// ```
    pub fn register_authorizer(
        &mut self,
        name: impl Into<String>,
        resource: Arc<dyn AuthorizeResource>,
    ) -> &mut Self {
        self.authorize_registry.insert(name, resource);
        self
    }

    /// Register the authorizer enforcing ability-only `#[authorize]` checks.
    ///
    /// An ability-only declaration (`#[authorize("users.edit")]`, no target)
    /// carries no resource id, so it resolves through this single ability gate
    /// instead of the per-resource registry. The gate typically bridges the
    /// Gate's permission fallback (see the `rustasea-auth` ability-gate adapter)
    /// and renders the same `403` envelope. Without a registered ability gate an
    /// ability-only declaration fails the build closed with
    /// [`RouteError::UnknownAuthorization`]:
    ///
    /// ```rust,ignore
    /// let gate = Arc::new(build_gate()); // permissions installed
    /// router.authorize_abilities(authorizer_for_abilities(gate));
    /// router.authorize_ability_meta(__RUSTASEA_AUTHORIZE_ABILITY_users_edit)
    ///       .get_action("/users/:id/edit", edit_user);
    /// ```
    pub fn authorize_abilities(&mut self, gate: Arc<dyn AuthorizeResource>) -> &mut Self {
        self.authorize_registry.set_ability(gate);
        self
    }

    /// Declare a record-level authorization from `#[authorize]` metadata.
    ///
    /// Consumes the doc-hidden `__RUSTASEA_AUTHORIZE_<Fn>` tuple emitted by the
    /// `#[authorize]` attribute (`(ability, resource_id)`) and attaches it to
    /// the route registered next. Like [`Router::middleware`] the declaration is
    /// sticky, so declare it inside a [`Router::group`] or immediately before
    /// the target route.
    pub fn authorize_meta(&mut self, meta: (&str, &str)) -> &mut Self {
        self.pending_authorizations.push(AuthorizeSpec::Resource {
            ability: meta.0.to_string(),
            resource: meta.1.to_string(),
        });
        self
    }

    /// Declare an ability-only authorization from `#[authorize]` metadata.
    ///
    /// Consumes the doc-hidden `__RUSTASEA_AUTHORIZE_ABILITY_<Fn>` string
    /// emitted by the ability-only `#[authorize("users.edit")]` form and
    /// attaches it to the route registered next. The name is enforced through
    /// the ability gate registered with [`Router::authorize_abilities`]; a route
    /// declaring one without that gate fails the build closed with
    /// [`RouteError::UnknownAuthorization`].
    pub fn authorize_ability_meta(&mut self, name: &str) -> &mut Self {
        self.pending_authorizations.push(AuthorizeSpec::Ability {
            name: name.to_string(),
        });
        self
    }
}
