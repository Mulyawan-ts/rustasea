//! Unit tests for the RBAC registry, roles, caching, and accessors.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use rustasea_cache::{MemoryStore, Store};

use super::*;
use crate::guard::AuthUser;

/// Build a registry with an `admin` (all) and `editor` (posts.edit) role.
fn registry() -> RbacRegistry {
    let mut registry = RbacRegistry::new();
    registry.define_permission("users.edit").expect("define");
    registry.define_permission("posts.edit").expect("define");
    registry.define_permission("posts.delete").expect("define");
    registry
        .define_role(Role::new("admin").with_permissions([
            "users.edit",
            "posts.edit",
            "posts.delete",
        ]))
        .expect("define role");
    registry
        .define_role(Role::new("editor").with_permissions(["posts.edit"]))
        .expect("define role");
    registry
}

/// Assign/has role round-trips, and `remove_role` reports the transition.
#[test]
fn assign_and_remove_role() {
    let mut registry = registry();
    assert!(!registry.has_role("u1", "editor"));
    registry.assign_role("u1", "editor").expect("assign");
    assert!(registry.has_role("u1", "editor"));
    assert_eq!(registry.roles_for("u1"), vec!["editor".to_string()]);
    assert!(registry.remove_role("u1", "editor").expect("remove"));
    assert!(!registry.remove_role("u1", "editor").expect("remove again"));
    assert!(registry.roles_for("u1").is_empty());
}

/// Permissions union across every role a user holds.
#[test]
fn permission_union_across_roles() {
    let mut registry = registry();
    registry.assign_role("u1", "editor").expect("assign");
    registry.assign_role("u1", "admin").expect("assign");
    let permissions = registry.permissions_for("u1");
    let expected: BTreeSet<String> = ["users.edit", "posts.edit", "posts.delete"]
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(permissions, expected);
    assert!(registry.has_permission("u1", "users.edit"));
    assert!(!registry.has_permission("u2", "users.edit"));
}

/// Revoking a role removes its permissions from the effective set.
#[test]
fn revoke_role_invalidates_permissions() {
    let mut registry = registry();
    registry.assign_role("u1", "admin").expect("assign");
    assert!(registry.has_permission("u1", "posts.delete"));
    registry.remove_role("u1", "admin").expect("remove");
    assert!(!registry.has_permission("u1", "posts.delete"));
}

/// `sync_roles` replaces the whole assignment.
#[test]
fn sync_roles_replaces_assignment() {
    let mut registry = registry();
    registry.assign_role("u1", "admin").expect("assign");
    registry
        .sync_roles("u1", &["editor".to_string()])
        .expect("sync");
    assert_eq!(registry.roles_for("u1"), vec!["editor".to_string()]);
    assert!(!registry.has_permission("u1", "users.edit"));
    assert!(registry.has_permission("u1", "posts.edit"));
    // Empty sync clears the assignment entirely.
    registry.sync_roles("u1", &[]).expect("sync empty");
    assert!(registry.roles_for("u1").is_empty());
}

/// Assigning an undefined role is a typed error.
#[test]
fn unknown_role_is_typed_error() {
    let mut registry = registry();
    let error = registry.assign_role("u1", "ghost").expect_err("must fail");
    assert_eq!(
        error,
        RbacError::UnknownRole {
            role: "ghost".to_string()
        }
    );
    assert_eq!(error.code(), "RbacError::UnknownRole");
    // A role grant of an undefined permission is also typed.
    let error = registry
        .grant_permission_to_role("admin", "ghost.edit")
        .expect_err("must fail");
    assert_eq!(
        error,
        RbacError::UnknownPermission {
            permission: "ghost.edit".to_string()
        }
    );
}

/// A cache-backed lookup hits on the second read and invalidates on write.
#[tokio::test]
async fn cache_hit_miss_and_invalidation() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let mut registry = registry()
        .with_cache(Arc::clone(&store))
        .with_cache_ttl(Duration::from_secs(60));
    registry.assign_role("u1", "admin").expect("assign");

    // First read is a miss that populates the cache.
    assert!(registry.has_permission_cached("u1", "users.edit").await);
    let raw = store.get("rbac:perms:u1").await.expect("get").is_some();
    assert!(raw, "first read must populate the cache");

    // Second read is served from the cache and agrees.
    assert!(registry.has_permission_cached("u1", "users.edit").await);

    // Revoking the role bumps the generation, so the cached entry is stale
    // and the next read recomputes to a denial.
    registry.remove_role("u1", "admin").expect("remove");
    assert!(!registry.has_permission_cached("u1", "users.edit").await);
}

/// `forget_cached_permissions` purges the entry eagerly.
#[tokio::test]
async fn forget_cached_permissions_purges_entry() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let mut registry = registry().with_cache(Arc::clone(&store));
    registry.assign_role("u1", "editor").expect("assign");
    assert!(registry.has_permission_cached("u1", "posts.edit").await);
    registry
        .forget_cached_permissions("u1")
        .await
        .expect("forget");
    assert!(store.get("rbac:perms:u1").await.expect("get").is_none());
}

/// A cache-less registry still answers correctly (degraded path).
#[tokio::test]
async fn cacheless_registry_degrades_to_memory() {
    let mut registry = registry();
    registry.assign_role("u1", "editor").expect("assign");
    assert!(registry.has_permission_cached("u1", "posts.edit").await);
    assert!(!registry.has_permission_cached("u1", "users.edit").await);
}

/// The `HasRoles` blanket impl keys on the principal id.
#[test]
fn has_roles_trait_keys_on_id() {
    let mut registry = registry();
    registry.assign_role("user-7", "editor").expect("assign");
    let user = AuthUser::new("user-7", Some("seven@example.com"), "session");
    assert!(user.has_role(&registry, "editor"));
    assert!(user.has_permission(&registry, "posts.edit"));
    assert!(!user.has_permission(&registry, "users.edit"));
    assert_eq!(user.roles(&registry), vec!["editor".to_string()]);
    assert_eq!(
        user.permissions(&registry),
        ["posts.edit".to_string()].into_iter().collect()
    );
}

/// The resolver bridge delegates to the in-memory check.
#[test]
fn permission_resolver_bridge() {
    let mut registry = registry();
    registry.assign_role("u1", "editor").expect("assign");
    let resolver: &dyn PermissionResolver = &registry;
    assert!(resolver.has_permission("u1", "posts.edit"));
    assert!(!resolver.has_permission("u1", "users.edit"));
}

/// Role builder helpers behave and are deterministic.
#[test]
fn role_builder_and_set_operations() {
    let mut role = Role::new("editor").with_permissions(["a", "b"]);
    assert!(role.has_permission("a"));
    assert!(role.revoke("a"));
    assert!(!role.revoke("a"));
    role.grant("c");
    let expected: BTreeSet<String> = ["b", "c"].into_iter().map(str::to_string).collect();
    assert_eq!(role.permissions(), &expected);
    assert_eq!(role.name(), "editor");
}
