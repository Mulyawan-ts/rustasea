//! Unit tests for the ability-based [`Gate`](super::Gate).
//!
//! Kept out of `gate/mod.rs` so the module stays within the 500-line budget.

use std::sync::Arc;

use super::*;
use crate::rbac::{PermissionResolver, RbacRegistry, Role};

/// A minimal resource carrying its author id.
struct Post {
    author_id: String,
}

fn post(author_id: &str) -> Post {
    Post {
        author_id: author_id.to_string(),
    }
}

fn user(id: &str) -> AuthUser {
    AuthUser::new(id, Some(format!("{id}@example.com")), "session")
}

/// `edit-post` allows the author and nobody else.
fn author_gate() -> Gate {
    let mut gate = Gate::new();
    gate.define("edit-post", |user, target| {
        let Some(post) = target.downcast_ref::<Post>() else {
            return false;
        };
        user.map(|u| u.id == post.author_id).unwrap_or(false)
    });
    gate
}

/// A registry where `user-1` holds a role granting `users.edit`.
fn permission_registry() -> Arc<RbacRegistry> {
    let mut registry = RbacRegistry::new();
    registry.define_permission("users.edit").expect("define");
    registry
        .define_role(Role::new("admin").with_permissions(["users.edit"]))
        .expect("role");
    registry.assign_role("user-1", "admin").expect("assign");
    Arc::new(registry)
}

/// A tiny resolver that grants a fixed set of abilities for one user id.
struct FixedResolver {
    user_id: String,
    abilities: Vec<String>,
}

impl PermissionResolver for FixedResolver {
    fn has_permission(&self, user_id: &str, ability: &str) -> bool {
        user_id == self.user_id && self.abilities.iter().any(|a| a == ability)
    }
}

/// Positive: the author may edit their own post.
#[test]
fn author_is_allowed_to_edit_post() {
    let gate = author_gate();
    let post = post("user-1");
    let author = user("user-1");
    assert!(gate.allows_for(Some(&author), "edit-post", &post));
    assert!(!gate.denies_for(Some(&author), "edit-post", &post));
    assert_eq!(
        gate.authorize_for(Some(&author), "edit-post", &post),
        Ok(())
    );
}

/// Negative: a non-author is denied with a typed `AuthorizationError`.
#[test]
fn non_author_is_denied() {
    let gate = author_gate();
    let post = post("user-1");
    let other = user("user-2");
    assert!(!gate.allows_for(Some(&other), "edit-post", &post));
    assert!(gate.denies_for(Some(&other), "edit-post", &post));
    assert_eq!(
        gate.authorize_for(Some(&other), "edit-post", &post),
        Err(AuthorizationError::Denied {
            ability: "edit-post".to_string()
        })
    );
}

/// A `before` hook bypasses the ability callback (super-admin override).
#[test]
fn before_hook_overrides_decision() {
    let mut gate = author_gate();
    gate.before(|user, _ability, _target| {
        if user.map(|u| u.id == "super-admin").unwrap_or(false) {
            Some(true)
        } else {
            None
        }
    });
    let post = post("user-1");
    let super_admin = user("super-admin");
    // Not the author, yet allowed because `before` short-circuits.
    assert!(gate.allows_for(Some(&super_admin), "edit-post", &post));
    assert_eq!(
        gate.authorize_for(Some(&super_admin), "edit-post", &post),
        Ok(())
    );
    // The bypass also covers an ability that was never defined.
    assert!(gate.allows_for(Some(&super_admin), "delete-post", &post));
}

/// An undefined ability fails closed for both `allows` and `authorize`.
#[test]
fn undefined_ability_fails_closed() {
    let gate = author_gate();
    let post = post("user-1");
    let author = user("user-1");
    assert!(!gate.allows_for(Some(&author), "delete-post", &post));
    assert!(gate.denies_for(Some(&author), "delete-post", &post));
    assert_eq!(
        gate.authorize_for(Some(&author), "delete-post", &post),
        Err(AuthorizationError::AbilityNotDefined {
            ability: "delete-post".to_string()
        })
    );
    assert_eq!(gate.abilities(), vec!["edit-post".to_string()]);
    assert!(gate.has("edit-post"));
}

/// The implicit entry points resolve the user through the installed resolver.
#[test]
fn implicit_user_resolves_via_resolver() {
    let gate = author_gate().with_user_resolver(|| Some(user("user-1")));
    let post = post("user-1");
    assert!(gate.allows("edit-post", &post));
    assert_eq!(gate.authorize("edit-post", &post), Ok(()));

    // No resolver installed => `None` user => fail closed.
    let bare = author_gate();
    assert!(bare.denies("edit-post", &post));
}

/// `after` hooks can rewrite the computed result.
#[test]
fn after_hook_rewrites_result() {
    let mut gate = author_gate();
    gate.after(|_user, _ability, _target, _result| false);
    let post = post("user-1");
    let author = user("user-1");
    assert!(!gate.allows_for(Some(&author), "edit-post", &post));
}

/// The typed error renders the documented `403` JSON envelope.
#[test]
fn error_renders_403_envelope() {
    let err = AuthorizationError::Denied {
        ability: "edit-post".to_string(),
    };
    assert_eq!(err.code(), "AuthorizationError::Denied");
    let response = err.into_response();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// A permission resolver allows an ability the Gate never defined.
#[test]
fn permission_resolver_allows_undefined_ability() {
    let gate = Gate::new().with_permissions(permission_registry());
    let holder = user("user-1");
    assert!(gate.allows_for(Some(&holder), "users.edit", &()));
    assert_eq!(gate.authorize_for(Some(&holder), "users.edit", &()), Ok(()));
    // A different user is denied; a different ability is denied.
    let other = user("user-2");
    assert!(!gate.allows_for(Some(&other), "users.edit", &()));
    assert!(!gate.allows_for(Some(&holder), "users.delete", &()));
}

/// A defined ability takes precedence over the permission resolver.
#[test]
fn defined_ability_takes_precedence_over_permission() {
    let mut gate = author_gate().with_permissions(permission_registry());
    // `edit-post` is defined and the policy denies a non-author even though
    // the resolver would have granted the same ability name.
    gate.define("users.edit", |_user, _target| false);
    let holder = user("user-1");
    assert!(!gate.allows_for(Some(&holder), "users.edit", &()));
}

/// No user present => the permission resolver is skipped (fail closed).
#[test]
fn permission_resolver_skipped_without_user() {
    let gate = Gate::new().with_permissions(permission_registry());
    assert!(!gate.allows_for(None, "users.edit", &()));
    assert_eq!(
        gate.authorize_for(None, "users.edit", &()),
        Err(AuthorizationError::AbilityNotDefined {
            ability: "users.edit".to_string()
        })
    );
}

/// With no resolver installed the undefined-ability behaviour is unchanged.
#[test]
fn resolver_absent_keeps_ability_not_defined() {
    let gate = author_gate();
    assert_eq!(
        gate.authorize_for(Some(&user("user-1")), "users.edit", &()),
        Err(AuthorizationError::AbilityNotDefined {
            ability: "users.edit".to_string()
        })
    );
}

/// A `before` hook still short-circuits before the permission resolver.
#[test]
fn before_hook_precedes_permission_resolver() {
    let mut gate = Gate::new().with_permissions(permission_registry());
    gate.before(|_user, ability, _target| (ability == "users.edit").then_some(false));
    let holder = user("user-1");
    assert!(!gate.allows_for(Some(&holder), "users.edit", &()));
}

/// A custom resolver implementation composes with the Gate.
#[test]
fn custom_resolver_composes() {
    let resolver: Arc<dyn PermissionResolver> = Arc::new(FixedResolver {
        user_id: "u9".to_string(),
        abilities: vec!["reports.view".to_string()],
    });
    let gate = Gate::new().with_permissions(resolver);
    assert!(gate.allows_for(Some(&user("u9")), "reports.view", &()));
    assert!(!gate.allows_for(Some(&user("u9")), "reports.export", &()));
}
