//! Role-based access control — roles & permissions with cached lookups.
//!
//! `spatie/laravel-permission` parity: users hold *roles*, roles hold
//! *permissions*, and a permission check is the union of the permissions of
//! every role the user holds. This sits alongside the ability-based
//! [`crate::gate`] — roles answer the coarse "what may this kind of user do?"
//! question, while Gate abilities and policies answer the fine-grained
//! "may this user do this to this record?" question. The two compose: the Gate
//! consults an optional [`PermissionResolver`] after its defined abilities.
//!
//! # Shape
//!
//! * [`Role`] — a named set of permissions, defined once at boot.
//! * [`RbacRegistry`] — the in-memory assignment store (`user_id → roles`) with
//!   an optional [`rustasea_cache::Store`] in front of permission lookups.
//! * [`HasRoles`] — ergonomic `user.has_permission(&registry, "users.edit")`
//!   accessors, blanket-implemented for [`crate::AuthUser`].
//! * [`PermissionResolver`] — the object-safe bridge the Gate calls.
//!
//! # Identity keying
//!
//! [`crate::AuthUser`] carries no roles field, so every lookup is keyed by the
//! principal's `id` string. The application resolves the roles for that id at
//! boot (or per request) and stores the assignment in the registry.
//!
//! # Caching
//!
//! When a store is installed with [`RbacRegistry::with_cache`], permission
//! lookups memoize the union under `rbac:perms:{user_id}` (the store's own key
//! prefix still applies). A role/permission write bumps a per-user epoch so a
//! stale entry is ignored on the next read; cache failures degrade silently to
//! the in-memory computation and never panic or widen access.

mod has_roles;
mod registry;
mod role;

#[cfg(test)]
mod tests;

pub use has_roles::HasRoles;
pub use registry::{PermissionResolver, RbacError, RbacRegistry};
pub use role::Role;
