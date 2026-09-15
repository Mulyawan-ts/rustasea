//! A named role carrying a set of permission names.

use std::collections::BTreeSet;

/// A named role and the permissions it grants.
///
/// Roles are defined once at boot and are immutable in shape thereafter — the
/// registry clones them when it evaluates a user's effective permissions. The
/// permission set is a [`BTreeSet`] so iteration is deterministic (stable logs,
/// stable serialized cache payloads, stable tests).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Role {
    /// Unique role name, e.g. `"admin"` or `"editor"`.
    pub name: String,
    /// Permission names this role grants, e.g. `"users.edit"`.
    pub permissions: BTreeSet<String>,
}

impl Role {
    /// Create a role with no permissions.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            permissions: BTreeSet::new(),
        }
    }

    /// Builder: attach an initial permission set.
    pub fn with_permissions<I, S>(mut self, permissions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.permissions
            .extend(permissions.into_iter().map(Into::into));
        self
    }

    /// Grant `permission` to this role (idempotent).
    pub fn grant(&mut self, permission: impl Into<String>) -> &mut Self {
        self.permissions.insert(permission.into());
        self
    }

    /// Revoke `permission`, returning whether it had been granted.
    pub fn revoke(&mut self, permission: &str) -> bool {
        self.permissions.remove(permission)
    }

    /// Whether this role grants `permission`.
    pub fn has_permission(&self, permission: &str) -> bool {
        self.permissions.contains(permission)
    }

    /// The role name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The permissions this role grants.
    pub fn permissions(&self) -> &BTreeSet<String> {
        &self.permissions
    }
}
