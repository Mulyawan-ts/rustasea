//! In-memory role/permission registry with an optional cache front.
//!
//! The registry owns the boot-time role catalog and the per-user role
//! assignment. Permission lookups union the permissions of every role a user
//! holds. When a [`rustasea_cache::Store`] is installed the async
//! `*_cached` entry points memoize the union under `rbac:perms:{user_id}`.
//!
//! # Why the cache is async-only
//!
//! The [`Store`] trait is async while the [`PermissionResolver`] the Gate calls
//! is synchronous (ability callbacks cannot await). The registry therefore
//! exposes two families: the sync in-memory lookups ([`RbacRegistry::has_permission`],
//! [`RbacRegistry::permissions_for`]) that back the resolver and [`crate::rbac::HasRoles`],
//! and the async store-backed lookups ([`RbacRegistry::has_permission_cached`],
//! [`RbacRegistry::permissions_for_cached`]) for request paths that can await.
//! A cache miss recomputes from memory and writes the union back with the
//! configured TTL.
//!
//! # Invalidation
//!
//! Every mutating call bumps a process-wide generation counter. A cached entry
//! records the generation it was written under; a read whose generation is
//! stale is treated as a miss and recomputed, so a write can never serve a
//! stale allow. [`RbacRegistry::forget_cached_permissions`] purges one user's
//! entry eagerly. Cache failures are swallowed and fall through to the
//! in-memory computation — a broken cache never blocks a check.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use rustasea_cache::Store;

use super::role::Role;

/// Cache key prefix for a user's effective permission set.
const PERMISSIONS_KEY_PREFIX: &str = "rbac:perms:";

/// Typed RBAC failure.
///
/// Follows the crate's `code()` convention (see
/// [`crate::gate::AuthorizationError`]); a variant name is stable and
/// machine-readable for logs and tests.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RbacError {
    /// A role was referenced that was never defined.
    #[error("unknown role: {role:?}")]
    UnknownRole {
        /// The undefined role name.
        role: String,
    },
    /// A permission was referenced that was never defined.
    #[error("unknown permission: {permission:?}")]
    UnknownPermission {
        /// The undefined permission name.
        permission: String,
    },
    /// A role or permission name was empty.
    #[error("role/permission names must not be empty")]
    EmptyName,
    /// An internal lock was poisoned by a panicking writer.
    #[error("rbac registry lock poisoned")]
    LockPoisoned,
    /// The backing cache store failed.
    #[error("rbac cache failure: {0}")]
    Cache(String),
}

impl RbacError {
    /// Stable machine-readable code, e.g. `RbacError::UnknownRole`.
    pub fn code(&self) -> String {
        let variant = match self {
            RbacError::UnknownRole { .. } => "UnknownRole",
            RbacError::UnknownPermission { .. } => "UnknownPermission",
            RbacError::EmptyName => "EmptyName",
            RbacError::LockPoisoned => "LockPoisoned",
            RbacError::Cache(_) => "Cache",
        };
        format!("RbacError::{variant}")
    }
}

/// Object-safe permission bridge the Gate consults after its defined abilities.
///
/// Implemented for [`RbacRegistry`] as a synchronous wrapper over the in-memory
/// check. Applications that want a cache-backed resolver can implement this
/// trait themselves (e.g. reading a request-scoped snapshot).
pub trait PermissionResolver: Send + Sync {
    /// Whether `user_id` holds `ability` through any assigned role.
    fn has_permission(&self, user_id: &str, ability: &str) -> bool;
}

/// Serializable cache payload recording the generation it was written under.
#[derive(Debug, Serialize, Deserialize)]
struct CachedPermissions {
    /// Registry generation at write time; a mismatch invalidates the entry.
    generation: u64,
    /// The user's effective permissions, sorted.
    permissions: Vec<String>,
}

/// In-memory role catalog and per-user role assignments.
///
/// Construct empty, define roles/permissions at boot, assign them to users, and
/// read effective permissions per request. Cloning is not supported — share one
/// registry behind an [`Arc`].
#[derive(Default)]
pub struct RbacRegistry {
    /// Role name → role definition.
    roles: RwLock<HashMap<String, Role>>,
    /// The catalog of defined permission names.
    permissions: RwLock<BTreeSet<String>>,
    /// User id → assigned role names.
    user_roles: RwLock<HashMap<String, BTreeSet<String>>>,
    /// Optional cache store fronting the permission lookups.
    cache: Option<Arc<dyn Store>>,
    /// TTL applied to cached permission sets.
    cache_ttl: Duration,
    /// Monotonic generation bumped on every mutation.
    generation: AtomicU64,
}

impl RbacRegistry {
    /// Create an empty registry with no roles, permissions, or cache.
    pub fn new() -> Self {
        Self {
            roles: RwLock::new(HashMap::new()),
            permissions: RwLock::new(BTreeSet::new()),
            user_roles: RwLock::new(HashMap::new()),
            cache: None,
            cache_ttl: Duration::from_secs(300),
            generation: AtomicU64::new(0),
        }
    }

    /// Install a cache store fronting the async permission lookups.
    ///
    /// The TTL defaults to five minutes; override with
    /// [`RbacRegistry::with_cache_ttl`].
    pub fn with_cache(mut self, store: Arc<dyn Store>) -> Self {
        self.cache = Some(store);
        self
    }

    /// Override the TTL applied to cached permission sets.
    pub fn with_cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;
        self
    }

    /// Bump the invalidation generation.
    fn bump(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Define (or replace) a role.
    pub fn define_role(&mut self, role: Role) -> Result<(), RbacError> {
        if role.name.trim().is_empty() {
            return Err(RbacError::EmptyName);
        }
        let mut roles = self.roles.write().map_err(|_| RbacError::LockPoisoned)?;
        roles.insert(role.name.clone(), role);
        drop(roles);
        self.bump();
        Ok(())
    }

    /// Define a permission name in the catalog.
    ///
    /// Idempotent; an empty name is rejected so a typo cannot create an
    /// unreachable permission.
    pub fn define_permission(&mut self, name: impl Into<String>) -> Result<(), RbacError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(RbacError::EmptyName);
        }
        let mut permissions = self
            .permissions
            .write()
            .map_err(|_| RbacError::LockPoisoned)?;
        permissions.insert(name);
        drop(permissions);
        self.bump();
        Ok(())
    }

    /// Grant `permission` to `role`.
    ///
    /// # Errors
    ///
    /// [`RbacError::UnknownRole`] when `role` is undefined and
    /// [`RbacError::UnknownPermission`] when `permission` was never defined.
    pub fn grant_permission_to_role(
        &mut self,
        role: &str,
        permission: &str,
    ) -> Result<(), RbacError> {
        if !self
            .permissions
            .read()
            .map_err(|_| RbacError::LockPoisoned)?
            .contains(permission)
        {
            return Err(RbacError::UnknownPermission {
                permission: permission.to_string(),
            });
        }
        let mut roles = self.roles.write().map_err(|_| RbacError::LockPoisoned)?;
        let role = roles.get_mut(role).ok_or_else(|| RbacError::UnknownRole {
            role: role.to_string(),
        })?;
        role.grant(permission);
        drop(roles);
        self.bump();
        Ok(())
    }

    /// Revoke `permission` from `role`, returning whether it had been granted.
    pub fn revoke_permission_from_role(
        &mut self,
        role: &str,
        permission: &str,
    ) -> Result<bool, RbacError> {
        let mut roles = self.roles.write().map_err(|_| RbacError::LockPoisoned)?;
        let role = roles.get_mut(role).ok_or_else(|| RbacError::UnknownRole {
            role: role.to_string(),
        })?;
        let removed = role.revoke(permission);
        drop(roles);
        self.bump();
        Ok(removed)
    }

    /// Assign `role` to `user_id`.
    ///
    /// # Errors
    ///
    /// [`RbacError::UnknownRole`] when the role was never defined.
    pub fn assign_role(&mut self, user_id: &str, role: &str) -> Result<(), RbacError> {
        if !self
            .roles
            .read()
            .map_err(|_| RbacError::LockPoisoned)?
            .contains_key(role)
        {
            return Err(RbacError::UnknownRole {
                role: role.to_string(),
            });
        }
        let mut user_roles = self
            .user_roles
            .write()
            .map_err(|_| RbacError::LockPoisoned)?;
        user_roles
            .entry(user_id.to_string())
            .or_default()
            .insert(role.to_string());
        drop(user_roles);
        self.bump();
        Ok(())
    }

    /// Remove `role` from `user_id`, returning whether it had been assigned.
    pub fn remove_role(&mut self, user_id: &str, role: &str) -> Result<bool, RbacError> {
        let mut user_roles = self
            .user_roles
            .write()
            .map_err(|_| RbacError::LockPoisoned)?;
        let removed = user_roles
            .get_mut(user_id)
            .map(|roles| roles.remove(role))
            .unwrap_or(false);
        drop(user_roles);
        if removed {
            self.bump();
        }
        Ok(removed)
    }

    /// Replace `user_id`'s role set with `roles`.
    ///
    /// # Errors
    ///
    /// [`RbacError::UnknownRole`] when any name is undefined; the assignment is
    /// left untouched in that case.
    pub fn sync_roles(&mut self, user_id: &str, roles: &[String]) -> Result<(), RbacError> {
        {
            let defined = self.roles.read().map_err(|_| RbacError::LockPoisoned)?;
            for role in roles {
                if !defined.contains_key(role) {
                    return Err(RbacError::UnknownRole { role: role.clone() });
                }
            }
        }
        let mut user_roles = self
            .user_roles
            .write()
            .map_err(|_| RbacError::LockPoisoned)?;
        if roles.is_empty() {
            user_roles.remove(user_id);
        } else {
            user_roles.insert(user_id.to_string(), roles.iter().cloned().collect());
        }
        drop(user_roles);
        self.bump();
        Ok(())
    }

    /// Whether `user_id` holds `role`.
    pub fn has_role(&self, user_id: &str, role: &str) -> bool {
        self.user_roles
            .read()
            .map(|roles| {
                roles
                    .get(user_id)
                    .map(|set| set.contains(role))
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    /// The role names assigned to `user_id`, sorted.
    pub fn roles_for(&self, user_id: &str) -> Vec<String> {
        self.user_roles
            .read()
            .map(|roles| {
                roles
                    .get(user_id)
                    .map(|set| set.iter().cloned().collect())
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    }

    /// The union of permissions granted by every role `user_id` holds.
    pub fn permissions_for(&self, user_id: &str) -> BTreeSet<String> {
        let assigned = match self.user_roles.read() {
            Ok(roles) => roles.get(user_id).cloned().unwrap_or_default(),
            Err(_) => return BTreeSet::new(),
        };
        let catalog = match self.roles.read() {
            Ok(roles) => roles,
            Err(_) => return BTreeSet::new(),
        };
        let mut effective = BTreeSet::new();
        for role_name in assigned {
            if let Some(role) = catalog.get(&role_name) {
                effective.extend(role.permissions.iter().cloned());
            }
        }
        effective
    }

    /// Whether `user_id` holds `permission` through any assigned role.
    pub fn has_permission(&self, user_id: &str, permission: &str) -> bool {
        self.permissions_for(user_id).contains(permission)
    }

    /// Cache-aware permission lookup: hit the store, else compute and populate.
    ///
    /// A missing store, a cache error, or a stale generation degrades silently
    /// to the in-memory computation.
    pub async fn has_permission_cached(&self, user_id: &str, permission: &str) -> bool {
        self.permissions_for_cached(user_id)
            .await
            .contains(permission)
    }

    /// Cache-aware effective-permission lookup.
    ///
    /// Reads `rbac:perms:{user_id}` from the store first; on a miss (or stale
    /// generation) it recomputes from memory and writes the union back with the
    /// configured TTL. Cache failures never propagate — the in-memory value is
    /// always returned.
    pub async fn permissions_for_cached(&self, user_id: &str) -> BTreeSet<String> {
        let Some(store) = self.cache.as_ref() else {
            return self.permissions_for(user_id);
        };
        let key = format!("{PERMISSIONS_KEY_PREFIX}{user_id}");
        let generation = self.generation.load(Ordering::SeqCst);
        if let Ok(Some(bytes)) = store.get(&key).await {
            if let Ok(cached) = serde_json::from_slice::<CachedPermissions>(&bytes) {
                if cached.generation == generation {
                    return cached.permissions.into_iter().collect();
                }
            }
        }
        let permissions = self.permissions_for(user_id);
        let payload = CachedPermissions {
            generation,
            permissions: permissions.iter().cloned().collect(),
        };
        if let Ok(bytes) = serde_json::to_vec(&payload) {
            let _ = store.put(&key, bytes, self.cache_ttl).await;
        }
        permissions
    }

    /// Purge the cached permission set for `user_id`.
    ///
    /// # Errors
    ///
    /// [`RbacError::Cache`] when the store rejects the delete.
    pub async fn forget_cached_permissions(&self, user_id: &str) -> Result<(), RbacError> {
        let Some(store) = self.cache.as_ref() else {
            return Ok(());
        };
        let key = format!("{PERMISSIONS_KEY_PREFIX}{user_id}");
        store
            .forget(&key)
            .await
            .map_err(|error| RbacError::Cache(error.to_string()))
    }

    /// The role definition registered under `name`, if any.
    pub fn role(&self, name: &str) -> Option<Role> {
        self.roles
            .read()
            .ok()
            .and_then(|roles| roles.get(name).cloned())
    }

    /// The defined permission names, sorted.
    pub fn permissions(&self) -> Vec<String> {
        self.permissions
            .read()
            .map(|permissions| permissions.iter().cloned().collect())
            .unwrap_or_default()
    }
}

impl PermissionResolver for RbacRegistry {
    /// Synchronous in-memory check backing the Gate fallback.
    fn has_permission(&self, user_id: &str, ability: &str) -> bool {
        RbacRegistry::has_permission(self, user_id, ability)
    }
}

impl std::fmt::Debug for RbacRegistry {
    /// Manual debug — the boxed store has no `Debug` impl.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let roles = self.roles.read().map(|r| r.len()).unwrap_or(0);
        let assigned_users = self.user_roles.read().map(|r| r.len()).unwrap_or(0);
        formatter
            .debug_struct("RbacRegistry")
            .field("roles", &roles)
            .field("assigned_users", &assigned_users)
            .field("has_cache", &self.cache.is_some())
            .finish()
    }
}
