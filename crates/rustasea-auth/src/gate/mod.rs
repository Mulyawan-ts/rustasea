//! Ability-based authorization Gate — Laravel `Illuminate\Auth\Access\Gate` parity.
//!
//! The guard layer ([`crate::guard`]) answers *"who is the caller?"*; this module
//! answers the separate question *"may that caller perform this action on this
//! resource?"*. Abilities are registered once at boot with [`Gate::define`] and
//! evaluated per request against the resolved [`AuthUser`] and a target resource
//! passed as `&dyn Any`.
//!
//! # Identity resolution
//!
//! Laravel's Gate resolves the current user from the auth manager when a check
//! does not receive one explicitly (`Gate::allows('update', $post)` implicitly
//! uses `Auth::user()`). The RustaSea guards are **async** (`Guard::user`), while
//! ability callbacks are **synchronous** — so a Gate cannot await a guard inline.
//! The chosen design therefore carries an optional, synchronous *user resolver*
//! closure installed at boot ([`Gate::with_user_resolver`]) that snapshots the
//! already-resolved principal (e.g. the `AuthUser` an `axum` middleware placed
//! in the request extensions).
//!
//! Two entry-point families exist:
//!
//! * [`Gate::allows`] / [`Gate::authorize`] — resolve the user through the
//!   installed resolver (Laravel-parity ergonomics for request handlers).
//! * [`Gate::allows_for`] / [`Gate::authorize_for`] — take the user explicitly,
//!   for stateless callers and deterministic tests.
//!
//! When no resolver is installed the implicit user is `None`; abilities that
//! require a user then fail closed (a `None` user never becomes a silent allow).
//!
//! # Interceptors
//!
//! [`Gate::before`] callbacks run *before* the ability lookup and may short
//! circuit the decision — returning `Some(true)` is the classic super-admin
//! bypass, `Some(false)` a hard deny, and `None` falls through. [`Gate::after`]
//! callbacks observe and may rewrite the result (audit logging, global
//! constraints). A `before` bypass applies even to abilities that are not
//! explicitly defined, mirroring `Gate::before(fn ($user) => $user->isAdmin())`.
//!
//! # Permission fallback
//!
//! When a [`PermissionResolver`] is installed with [`Gate::with_permissions`]
//! the evaluation chain gains a role/permission step after the defined
//! abilities: **before hooks → defined ability/policy → permission resolver →
//! after hooks**. An ability registered with [`Gate::define`] always wins; the
//! resolver only fills the gap for an ability the Gate does not define, so the
//! classic `#[authorize("users.edit")]` permission name resolves through the
//! RBAC registry while record-level policies keep precedence. The resolver is
//! skipped when no user is present, and a `None` result still fails closed.
//!
//! # Error surface
//!
//! [`Gate::authorize`] returns a dedicated [`AuthorizationError`] with two
//! fail-closed variants (`Denied`, `AbilityNotDefined`). Following the
//! [`crate::throttle::registry::LimiterError`] precedent this module owns its
//! typed error instead of adding a variant to [`crate::error::AuthError`], so the
//! crate's `code()`/`hint()` wire contract stays untouched; [`AuthorizationError`]
//! reuses the same conventions and renders the `403` JSON envelope documented in
//! `api-auth.md`.
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use thiserror::Error;

use crate::guard::AuthUser;
use crate::rbac::PermissionResolver;

/// Object-safe ability callback: `(user, target) -> allowed`.
///
/// `Send + Sync` so a [`Gate`] can live inside `AppState`; the higher-ranked
/// lifetimes let one closure serve every borrow of the user and target.
pub type AbilityCallback =
    dyn for<'a, 'b> Fn(Option<&'a AuthUser>, &'b dyn Any) -> bool + Send + Sync;

/// Object-safe `before` interceptor: `(user, ability, target) -> Option<bool>`.
///
/// `Some(decision)` short-circuits the check; `None` falls through to the
/// ability callback (and any remaining `before` hooks).
pub type BeforeCallback = dyn for<'a, 'b, 'c> Fn(Option<&'a AuthUser>, &'b str, &'c dyn Any) -> Option<bool>
    + Send
    + Sync;

/// Object-safe `after` interceptor: `(user, ability, target, result) -> bool`.
///
/// Receives the result computed so far and returns the (possibly rewritten)
/// final result.
pub type AfterCallback =
    dyn for<'a, 'b, 'c> Fn(Option<&'a AuthUser>, &'b str, &'c dyn Any, bool) -> bool + Send + Sync;

/// Synchronous resolver for the implicit current user.
type UserResolver = dyn Fn() -> Option<AuthUser> + Send + Sync;

/// Typed authorization failure returned by [`Gate::authorize`].
///
/// Distinct from [`crate::error::AuthError`] (authentication) and follows the
/// crate's `code()`/`hint()` conventions plus the `errors[]` `403` envelope.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthorizationError {
    /// No ability is registered under the requested name — fail closed.
    #[error("ability not defined: {ability:?}")]
    AbilityNotDefined {
        /// Ability name the caller asked for.
        ability: String,
    },
    /// The ability is defined but the resolved user is not permitted.
    #[error("not authorized for ability {ability:?}")]
    Denied {
        /// Ability name that was denied.
        ability: String,
    },
}

impl AuthorizationError {
    /// Stable machine-readable code, e.g. `AuthorizationError::Denied`.
    pub fn code(&self) -> String {
        let variant = match self {
            AuthorizationError::AbilityNotDefined { .. } => "AbilityNotDefined",
            AuthorizationError::Denied { .. } => "Denied",
        };
        format!("AuthorizationError::{variant}")
    }

    /// Short user-facing remediation hint.
    pub fn hint(&self) -> &'static str {
        match self {
            AuthorizationError::AbilityNotDefined { .. } => {
                "Register the ability with Gate::define before checking it."
            }
            AuthorizationError::Denied { .. } => {
                "Request an action the current user is allowed to perform."
            }
        }
    }
}

/// Render an [`AuthorizationError`] as the documented `403` JSON envelope.
///
/// Mirrors `crate::csrf::csrf_error_response`: `{"errors":[{ "status", "code",
/// "title", "detail" }]}`. Both variants map to `403 Forbidden` — an undefined
/// ability is a server-side wiring gap, but it must never widen access, so it
/// fails closed with the same status as an explicit denial.
impl IntoResponse for AuthorizationError {
    fn into_response(self) -> Response {
        let (title, detail) = match &self {
            AuthorizationError::AbilityNotDefined { ability } => (
                "Ability not defined",
                format!("No authorization ability is registered under {ability:?}."),
            ),
            AuthorizationError::Denied { ability } => (
                "Forbidden",
                format!("You are not authorized to perform {ability:?}."),
            ),
        };
        let body = serde_json::json!({
            "errors": [{
                "status": "403",
                "code": self.code(),
                "title": title,
                "detail": detail,
            }]
        });
        (StatusCode::FORBIDDEN, Json(body)).into_response()
    }
}

/// Registry of named abilities plus `before`/`after` interceptor hooks.
///
/// Abilities and hooks are registered during boot (the `&mut self` setters) and
/// read afterwards through the `&self` checks, so no interior locking is needed
/// and a poisoned lock cannot occur. Per ADR-0007 the `Gate` is an owned value
/// passed explicitly (typically behind an `Arc`), never a process-wide singleton.
#[derive(Default)]
pub struct Gate {
    /// Named ability callbacks (`ability -> callback`).
    abilities: HashMap<String, Arc<AbilityCallback>>,
    /// `before` interceptors, evaluated in registration order.
    before: Vec<Arc<BeforeCallback>>,
    /// `after` interceptors, applied in registration order.
    after: Vec<Arc<AfterCallback>>,
    /// Optional synchronous resolver for the implicit current user.
    user_resolver: Option<Arc<UserResolver>>,
    /// Optional role/permission resolver consulted after defined abilities.
    permission_resolver: Option<Arc<dyn PermissionResolver>>,
}

impl Gate {
    /// Create an empty gate with no abilities, hooks, or user resolver.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) the callback for a named ability.
    ///
    /// The callback receives the resolved user (`None` when unauthenticated)
    /// and the target resource; returning `true` permits the action. Re-defining
    /// an ability replaces the previous callback, matching Laravel's
    /// `Gate::define` last-wins semantics.
    pub fn define<F>(&mut self, ability: &str, callback: F) -> &mut Self
    where
        F: for<'a, 'b> Fn(Option<&'a AuthUser>, &'b dyn Any) -> bool + Send + Sync + 'static,
    {
        self.abilities
            .insert(ability.to_string(), Arc::new(callback));
        self
    }

    /// Register a `before` interceptor that may short-circuit any check.
    ///
    /// Return `Some(true)` to force-allow (super-admin bypass), `Some(false)` to
    /// force-deny, or `None` to fall through. Hooks run in registration order and
    /// the first non-`None` result wins.
    pub fn before<F>(&mut self, callback: F) -> &mut Self
    where
        F: for<'a, 'b, 'c> Fn(Option<&'a AuthUser>, &'b str, &'c dyn Any) -> Option<bool>
            + Send
            + Sync
            + 'static,
    {
        self.before.push(Arc::new(callback));
        self
    }

    /// Register an `after` interceptor that may rewrite the computed result.
    ///
    /// Hooks run in registration order after the ability callback; each receives
    /// the result so far and returns the result passed to the next hook (and
    /// ultimately to the caller).
    pub fn after<F>(&mut self, callback: F) -> &mut Self
    where
        F: for<'a, 'b, 'c> Fn(Option<&'a AuthUser>, &'b str, &'c dyn Any, bool) -> bool
            + Send
            + Sync
            + 'static,
    {
        self.after.push(Arc::new(callback));
        self
    }

    /// Install the synchronous resolver used by the implicit-user entry points.
    ///
    /// The closure is typically `move || request.extensions().get::<AuthUser>()`
    /// so `allows`/`authorize` see the principal an auth middleware resolved.
    pub fn with_user_resolver<F>(mut self, resolver: F) -> Self
    where
        F: Fn() -> Option<AuthUser> + Send + Sync + 'static,
    {
        self.user_resolver = Some(Arc::new(resolver));
        self
    }

    /// Install a role/permission resolver consulted after defined abilities.
    ///
    /// The resolver (typically an [`crate::rbac::RbacRegistry`]) answers
    /// permission-name checks for abilities the Gate does not define, so
    /// `#[authorize("users.edit")]` and `Gate::allows("users.edit", &())` resolve
    /// through RBAC. It is skipped when no user is present; an ability
    /// registered with [`Gate::define`] always takes precedence.
    pub fn with_permissions(mut self, resolver: Arc<dyn PermissionResolver>) -> Self {
        self.permission_resolver = Some(resolver);
        self
    }

    /// Whether an ability is registered under `name`.
    pub fn has(&self, name: &str) -> bool {
        self.abilities.contains_key(name)
    }

    /// The registered ability names, sorted for deterministic iteration.
    pub fn abilities(&self) -> Vec<String> {
        let mut names: Vec<String> = self.abilities.keys().cloned().collect();
        names.sort();
        names
    }

    /// Whether the resolved user is allowed to perform `ability` on `target`.
    ///
    /// Resolves the user through the installed resolver; an undefined ability or
    /// a `None` user fails closed (`false`).
    pub fn allows(&self, ability: &str, target: &dyn Any) -> bool {
        let user = self.resolve_user();
        self.allows_for(user.as_ref(), ability, target)
    }

    /// [`Gate::allows`] with an explicit user (stateless / test entry point).
    pub fn allows_for(&self, user: Option<&AuthUser>, ability: &str, target: &dyn Any) -> bool {
        self.evaluate(user, ability, target).unwrap_or(false)
    }

    /// Whether the resolved user is denied `ability` on `target`.
    ///
    /// The complement of [`Gate::allows`], so an undefined ability or missing
    /// user is `true` (fail closed).
    pub fn denies(&self, ability: &str, target: &dyn Any) -> bool {
        !self.allows(ability, target)
    }

    /// [`Gate::denies`] with an explicit user.
    pub fn denies_for(&self, user: Option<&AuthUser>, ability: &str, target: &dyn Any) -> bool {
        !self.allows_for(user, ability, target)
    }

    /// Authorize the resolved user, returning a typed error on denial.
    ///
    /// `Ok(())` when allowed; [`AuthorizationError::Denied`] when the ability
    /// exists but the user is not permitted; [`AuthorizationError::AbilityNotDefined`]
    /// when no callback is registered.
    pub fn authorize(&self, ability: &str, target: &dyn Any) -> Result<(), AuthorizationError> {
        let user = self.resolve_user();
        self.authorize_for(user.as_ref(), ability, target)
    }

    /// [`Gate::authorize`] with an explicit user.
    pub fn authorize_for(
        &self,
        user: Option<&AuthUser>,
        ability: &str,
        target: &dyn Any,
    ) -> Result<(), AuthorizationError> {
        match self.evaluate(user, ability, target) {
            Some(true) => Ok(()),
            Some(false) => Err(AuthorizationError::Denied {
                ability: ability.to_string(),
            }),
            None => Err(AuthorizationError::AbilityNotDefined {
                ability: ability.to_string(),
            }),
        }
    }

    /// Resolve the implicit user, if a resolver is installed.
    fn resolve_user(&self) -> Option<AuthUser> {
        self.user_resolver.as_ref().and_then(|resolver| resolver())
    }

    /// Core evaluation: `before` hooks → defined ability → permission → `after`.
    ///
    /// Returns `None` only when the ability is undefined, no `before` hook
    /// short-circuited, and no permission resolver produced a decision — the
    /// callers translate that into a fail-closed denial. The permission step is
    /// skipped when no user is present or no resolver is installed.
    fn evaluate(&self, user: Option<&AuthUser>, ability: &str, target: &dyn Any) -> Option<bool> {
        for before in &self.before {
            if let Some(decision) = before(user, ability, target) {
                return Some(decision);
            }
        }
        let mut result = match self.abilities.get(ability) {
            Some(callback) => callback(user, target),
            None => match (self.permission_resolver.as_ref(), user) {
                (Some(resolver), Some(user)) => resolver.has_permission(&user.id, ability),
                _ => return None,
            },
        };
        for after in &self.after {
            result = after(user, ability, target, result);
        }
        Some(result)
    }
}

impl std::fmt::Debug for Gate {
    /// Manual debug — the boxed callbacks have no `Debug` impl.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gate")
            .field("abilities", &self.abilities())
            .field("before", &self.before.len())
            .field("after", &self.after.len())
            .field("has_user_resolver", &self.user_resolver.is_some())
            .field(
                "has_permission_resolver",
                &self.permission_resolver.is_some(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests;
