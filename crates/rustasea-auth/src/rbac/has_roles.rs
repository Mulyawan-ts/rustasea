//! `HasRoles` — ergonomic role/permission accessors for an authenticated user.
//!
//! [`crate::AuthUser`] carries no roles field, so the accessors take the
//! [`RbacRegistry`] explicitly and key every lookup on the principal's `id`.
//! This mirrors Laravel's `$user->hasRole('admin')` / `$user->hasPermissionTo(...)`
//! ergonomics without coupling the principal type to the RBAC store.

use crate::guard::AuthUser;
use crate::rbac::registry::RbacRegistry;

/// Role/permission accessors for a principal.
///
/// Blanket-implemented for [`AuthUser`] using its `id` as the registry key.
/// Applications may implement it for their own richer user models by delegating
/// to the same registry.
pub trait HasRoles {
    /// The registry key identifying this principal (`AuthUser::id`).
    fn rbac_id(&self) -> &str;

    /// Whether this principal holds `role`.
    fn has_role(&self, registry: &RbacRegistry, role: &str) -> bool {
        registry.has_role(self.rbac_id(), role)
    }

    /// Whether this principal holds `permission` through any assigned role.
    fn has_permission(&self, registry: &RbacRegistry, permission: &str) -> bool {
        registry.has_permission(self.rbac_id(), permission)
    }

    /// The role names assigned to this principal, sorted.
    fn roles(&self, registry: &RbacRegistry) -> Vec<String> {
        registry.roles_for(self.rbac_id())
    }

    /// The effective permission set for this principal, sorted.
    fn permissions(&self, registry: &RbacRegistry) -> std::collections::BTreeSet<String> {
        registry.permissions_for(self.rbac_id())
    }
}

impl HasRoles for AuthUser {
    /// Key RBAC lookups on the principal's `id`.
    fn rbac_id(&self) -> &str {
        &self.id
    }
}
