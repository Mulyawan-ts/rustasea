//! Model policy registration and resolution — Laravel `Illuminate\Auth\Access`
//! Model Policies parity.
//!
//! The [`crate::gate`] module answers *"is this ability allowed?"* through
//! closures registered by name. Policies are the complementary, model-centric
//! layer: a [`Policy<T>`] bundles the authorization rules for one resource type
//! (`Post` → `PostPolicy`) and exposes Laravel's standard CRUD action methods.
//! [`PolicyRegistry`] maps each resource type to its policy and resolves the
//! right policy from a *resource instance* at request time, so a single ability
//! name (`"update"`) dispatches to the policy of whatever type the target is.
//!
//! # Standard action → ability mapping
//!
//! | Ability (Laravel) | `Policy` method      | Receives resource | Meaning                  |
//! |-------------------|----------------------|-------------------|--------------------------|
//! | `viewAny`         | [`Policy::view_any`] | no                | list/browse a collection |
//! | `view`            | [`Policy::view`]     | yes               | inspect one record       |
//! | `create`          | [`Policy::create`]   | no                | create a new record      |
//! | `update`          | [`Policy::update`]   | yes               | modify one record        |
//! | `delete`          | [`Policy::delete`]   | yes               | remove one record        |
//!
//! [`PolicyAction::from_ability`] performs this mapping, accepting both the
//! camelCase Laravel names (`viewAny`) and their snake_case Rust spellings
//! (`view_any`). An ability name outside the standard set is never a silent
//! allow: [`PolicyRegistry::authorize`] returns
//! [`AuthorizationError::AbilityNotDefined`].
//!
//! # Fail-closed defaults
//!
//! Every [`Policy`] method defaults to `false`, so a policy that forgets to
//! implement an action denies it. A resource type with no registered policy
//! also fails closed ([`AuthorizationError::AbilityNotDefined`]). The registry
//! never panics on a poisoned lock — the operation becomes a no-op and the
//! check denies.
//!
//! # Composition with the Gate
//!
//! [`PolicyRegistry`] is a sibling of [`Gate`] (both live in `AppState` per
//! ADR-0007). Callers can evaluate directly with [`PolicyRegistry::authorize`]
//! / [`PolicyRegistry::can`], or bridge the registry into a [`Gate`] with
//! [`PolicyRegistry::install`], which registers one dispatcher ability per
//! standard action so `gate.authorize_for(user, "update", &post)` resolves
//! `PostPolicy`. Either path returns the shared [`AuthorizationError`] owned by
//! [`crate::gate`] — this module defines no error type of its own.
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, RwLock};

use crate::gate::{AuthorizationError, Gate};
use crate::guard::AuthUser;

/// The standard Laravel policy actions, each mapped to an ability name.
///
/// The five variants are the canonical CRUD surface; [`PolicyAction::as_str`]
/// is the ability name a [`Gate`] or [`PolicyRegistry`] is queried with, and
/// [`PolicyAction::requires_resource`] distinguishes the actions that operate
/// on a single record (`view`/`update`/`delete`) from the collection-level ones
/// (`viewAny`/`create`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PolicyAction {
    /// Ability `viewAny` — browse a collection (no resource).
    ViewAny,
    /// Ability `view` — inspect a single record.
    View,
    /// Ability `create` — create a new record (no resource).
    Create,
    /// Ability `update` — modify a single record.
    Update,
    /// Ability `delete` — remove a single record.
    Delete,
}

impl PolicyAction {
    /// Every standard action, in Laravel's documented order.
    pub const ALL: [PolicyAction; 5] = [
        PolicyAction::ViewAny,
        PolicyAction::View,
        PolicyAction::Create,
        PolicyAction::Update,
        PolicyAction::Delete,
    ];

    /// The canonical ability name (camelCase, matching Laravel method names).
    pub fn as_str(self) -> &'static str {
        match self {
            PolicyAction::ViewAny => "viewAny",
            PolicyAction::View => "view",
            PolicyAction::Create => "create",
            PolicyAction::Update => "update",
            PolicyAction::Delete => "delete",
        }
    }

    /// Map an ability name onto an action, or `None` for a non-standard name.
    ///
    /// Accepts the camelCase Laravel spelling (`viewAny`) and the snake_case
    /// Rust spelling (`view_any`); every other string is unknown and therefore
    /// fails closed at the [`PolicyRegistry`] boundary.
    pub fn from_ability(ability: &str) -> Option<Self> {
        match ability {
            "viewAny" | "view_any" => Some(PolicyAction::ViewAny),
            "view" => Some(PolicyAction::View),
            "create" => Some(PolicyAction::Create),
            "update" => Some(PolicyAction::Update),
            "delete" => Some(PolicyAction::Delete),
            _ => None,
        }
    }

    /// Whether this action evaluates a concrete resource instance.
    ///
    /// `view`/`update`/`delete` receive the target; `viewAny`/`create` do not
    /// (the registry still resolves the policy from the target's type).
    pub fn requires_resource(self) -> bool {
        matches!(
            self,
            PolicyAction::View | PolicyAction::Update | PolicyAction::Delete
        )
    }
}

/// Authorization rules for a single resource type `T` (Laravel `PostPolicy`).
///
/// Implement this trait per model and register the value with
/// [`PolicyRegistry::register`]. Every method defaults to `false` (fail
/// closed), so a policy only opts *in* to the actions it supports. The
/// methods are synchronous, matching the [`crate::gate`] callback contract.
///
/// # Example
///
/// ```rust
/// use rustasea_auth::{AuthUser, Policy};
///
/// /// A blog post owned by one author.
/// struct Post {
///     author_id: String,
/// }
///
/// /// Only the author may update their own post.
/// struct PostPolicy;
///
/// impl Policy<Post> for PostPolicy {
///     fn update(&self, user: Option<&AuthUser>, resource: &Post) -> bool {
///         user.map(|u| u.id == resource.author_id).unwrap_or(false)
///     }
/// }
/// ```
pub trait Policy<T: 'static>: Send + Sync {
    /// Ability `viewAny`: may the user browse a collection of `T`?
    ///
    /// Defaults to `false` (fail closed).
    fn view_any(&self, _user: Option<&AuthUser>) -> bool {
        false
    }

    /// Ability `view`: may the user inspect this specific `resource`?
    ///
    /// Defaults to `false` (fail closed).
    fn view(&self, _user: Option<&AuthUser>, _resource: &T) -> bool {
        false
    }

    /// Ability `create`: may the user create a new `T`?
    ///
    /// Defaults to `false` (fail closed).
    fn create(&self, _user: Option<&AuthUser>) -> bool {
        false
    }

    /// Ability `update`: may the user modify this specific `resource`?
    ///
    /// Defaults to `false` (fail closed).
    fn update(&self, _user: Option<&AuthUser>, _resource: &T) -> bool {
        false
    }

    /// Ability `delete`: may the user remove this specific `resource`?
    ///
    /// Defaults to `false` (fail closed).
    fn delete(&self, _user: Option<&AuthUser>, _resource: &T) -> bool {
        false
    }
}

/// Object-safe, type-erased view of a [`Policy<T>`] stored in the registry.
///
/// Private: the only way in is [`PolicyRegistry::register`], which wraps a
/// concrete [`Policy<T>`] in a [`PolicyAdapter`] that downcasts the `&dyn Any`
/// target back to `&T` before invoking the policy method.
trait ErasedPolicy: Send + Sync {
    /// Evaluate `action` for `user` against `target` (downcast when required).
    fn check(&self, action: PolicyAction, user: Option<&AuthUser>, target: &dyn Any) -> bool;

    /// Fully-qualified Rust type name of the resource this policy governs.
    fn resource_type(&self) -> &'static str;
}

/// Adapter pairing a concrete [`Policy<T>`] with the `T` it authorizes.
///
/// The `PhantomData<fn() -> T>` marker records `T` without owning a value, so
/// the adapter is `Send + Sync` whenever the policy is, and `T` needs no
/// `Clone`/`Default` bound.
struct PolicyAdapter<T, P> {
    policy: P,
    _marker: PhantomData<fn() -> T>,
}

impl<T: 'static, P: Policy<T>> PolicyAdapter<T, P> {
    /// Wrap `policy`, binding it to resource type `T`.
    fn new(policy: P) -> Self {
        Self {
            policy,
            _marker: PhantomData,
        }
    }
}

impl<T: 'static, P: Policy<T>> ErasedPolicy for PolicyAdapter<T, P> {
    /// Dispatch `action` onto the wrapped policy.
    ///
    /// Resource-bearing actions downcast `target` to `&T`; a type mismatch
    /// (the caller passed the wrong resource) fails closed with `false`.
    fn check(&self, action: PolicyAction, user: Option<&AuthUser>, target: &dyn Any) -> bool {
        match action {
            PolicyAction::ViewAny => self.policy.view_any(user),
            PolicyAction::Create => self.policy.create(user),
            PolicyAction::View | PolicyAction::Update | PolicyAction::Delete => {
                let Some(resource) = target.downcast_ref::<T>() else {
                    return false;
                };
                match action {
                    PolicyAction::View => self.policy.view(user, resource),
                    PolicyAction::Update => self.policy.update(user, resource),
                    PolicyAction::Delete => self.policy.delete(user, resource),
                    // Unreachable: the outer arm already matched the
                    // resource-bearing actions; kept fail-closed for clarity.
                    PolicyAction::ViewAny | PolicyAction::Create => false,
                }
            }
        }
    }

    fn resource_type(&self) -> &'static str {
        std::any::type_name::<T>()
    }
}

/// Maps resource types to their [`Policy`] and resolves the right policy from a
/// resource instance (Laravel's `Gate` policy auto-discovery).
///
/// Register policies once at boot with [`PolicyRegistry::register`]; the map is
/// keyed by [`TypeId`] and lives behind an `Arc<RwLock<_>>`, so the registry is
/// cheap to [`Clone`] (handles share one map) and safe to read concurrently at
/// request time. Per ADR-0007 it is an owned value in `AppState`, never a
/// global.
///
/// # Lifecycle
///
/// * **Register at boot, resolve at request time.** `register` takes `&self`
///   via interior mutability, but the intended sequence is populate → query.
/// * **Fail closed.** An unregistered type or a non-standard ability is a typed
///   [`AuthorizationError::AbilityNotDefined`]; a policy denial is
///   [`AuthorizationError::Denied`]. A poisoned lock yields a no-op and a
///   denial, never a panic.
pub struct PolicyRegistry {
    /// Type-erased policies keyed by the resource's [`TypeId`].
    policies: Arc<RwLock<HashMap<TypeId, Arc<dyn ErasedPolicy>>>>,
}

impl PolicyRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) the policy for resource type `T`.
    ///
    /// Last-wins, mirroring [`Gate::define`]. A poisoned lock makes this a
    /// no-op rather than a panic; the affected type then fails closed.
    pub fn register<T: 'static, P: Policy<T> + 'static>(&self, policy: P) -> &Self {
        if let Ok(mut map) = self.policies.write() {
            map.insert(
                TypeId::of::<T>(),
                Arc::new(PolicyAdapter::<T, P>::new(policy)),
            );
        }
        self
    }

    /// Whether a policy is registered for resource type `T`.
    pub fn contains<T: 'static>(&self) -> bool {
        self.policies
            .read()
            .map(|map| map.contains_key(&TypeId::of::<T>()))
            .unwrap_or(false)
    }

    /// Number of registered policies.
    pub fn len(&self) -> usize {
        self.policies
            .read()
            .map(|map| map.len())
            .unwrap_or_default()
    }

    /// Whether the registry holds no policies.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The governed resource type names, sorted for deterministic iteration.
    pub fn resource_types(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .policies
            .read()
            .map(|map| {
                map.values()
                    .map(|policy| policy.resource_type().to_string())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// Whether `user` may perform `ability` on `target` (`true`/`false`).
    ///
    /// Convenience over [`PolicyRegistry::authorize`] for boolean call sites;
    /// every failure (unknown ability, unregistered type, denial) is `false`.
    pub fn can(&self, user: Option<&AuthUser>, target: &dyn Any, ability: &str) -> bool {
        self.authorize(user, target, ability).is_ok()
    }

    /// Whether `user` is denied `ability` on `target` (complement of `can`).
    pub fn denies(&self, user: Option<&AuthUser>, target: &dyn Any, ability: &str) -> bool {
        !self.can(user, target, ability)
    }

    /// Authorize `user` for `ability` on `target`, returning a typed error.
    ///
    /// Resolves the policy by the target's [`TypeId`] and dispatches the mapped
    /// [`PolicyAction`].
    ///
    /// # Errors
    ///
    /// * [`AuthorizationError::AbilityNotDefined`] — `ability` is not a standard
    ///   action, or no policy is registered for the target's type.
    /// * [`AuthorizationError::Denied`] — a policy exists but returned `false`.
    pub fn authorize(
        &self,
        user: Option<&AuthUser>,
        target: &dyn Any,
        ability: &str,
    ) -> Result<(), AuthorizationError> {
        let action = PolicyAction::from_ability(ability).ok_or_else(|| {
            AuthorizationError::AbilityNotDefined {
                ability: ability.to_string(),
            }
        })?;
        let policy = self.resolve(&target.type_id()).ok_or_else(|| {
            AuthorizationError::AbilityNotDefined {
                ability: ability.to_string(),
            }
        })?;
        if policy.check(action, user, target) {
            Ok(())
        } else {
            Err(AuthorizationError::Denied {
                ability: ability.to_string(),
            })
        }
    }

    /// Register one dispatcher ability per standard action into `gate`.
    ///
    /// After installing, `gate.authorize_for(user, "update", &post)` resolves
    /// `PostPolicy` through this registry (dispatch is by the target's type, so
    /// one `"update"` ability serves every registered resource). This
    /// **overwrites** any pre-existing ability of the same name, matching
    /// [`Gate::define`] last-wins semantics.
    pub fn install(&self, gate: &mut Gate) {
        for action in PolicyAction::ALL {
            let registry = self.clone();
            gate.define(action.as_str(), move |user, target| {
                registry.can(user, target, action.as_str())
            });
        }
    }

    /// Look up the erased policy registered for `type_id`.
    fn resolve(&self, type_id: &TypeId) -> Option<Arc<dyn ErasedPolicy>> {
        self.policies
            .read()
            .ok()
            .and_then(|map| map.get(type_id).cloned())
    }
}

impl Default for PolicyRegistry {
    /// An empty registry with a fresh (unshared) policy map.
    fn default() -> Self {
        Self {
            policies: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Clone for PolicyRegistry {
    /// Clones share the same policy map (`policies` is `Arc`d).
    fn clone(&self) -> Self {
        Self {
            policies: Arc::clone(&self.policies),
        }
    }
}

impl std::fmt::Debug for PolicyRegistry {
    /// Manual debug — the erased policies have no `Debug` impl.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolicyRegistry")
            .field("policies", &self.resource_types())
            .finish()
    }
}

#[cfg(test)]
mod tests;
