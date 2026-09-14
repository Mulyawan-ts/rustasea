//! Unit tests for model policy registration and resolution.

use super::*;

/// A blog post owned by one author.
struct Post {
    author_id: String,
    published: bool,
}

fn post(author_id: &str) -> Post {
    Post {
        author_id: author_id.to_string(),
        published: false,
    }
}

fn user(id: &str) -> AuthUser {
    AuthUser::new(id, Some(format!("{id}@example.com")), "session")
}

/// Author-only update; `view` also allows a published post; `viewAny`/
/// `create` allow any authenticated user; `delete` is left at the default.
struct PostPolicy;

impl Policy<Post> for PostPolicy {
    fn view_any(&self, user: Option<&AuthUser>) -> bool {
        user.is_some()
    }

    fn view(&self, user: Option<&AuthUser>, resource: &Post) -> bool {
        resource.published || user.map(|u| u.id == resource.author_id).unwrap_or(false)
    }

    fn create(&self, user: Option<&AuthUser>) -> bool {
        user.is_some()
    }

    fn update(&self, user: Option<&AuthUser>, resource: &Post) -> bool {
        user.map(|u| u.id == resource.author_id).unwrap_or(false)
    }
}

/// A registry with `PostPolicy` registered for `Post`.
fn registry() -> PolicyRegistry {
    let registry = PolicyRegistry::new();
    registry.register::<Post, _>(PostPolicy);
    registry
}

/// The registry resolves the policy by resource type.
#[test]
fn policy_is_registered_and_resolved_by_type() {
    let registry = registry();
    assert!(registry.contains::<Post>());
    assert_eq!(registry.len(), 1);
    assert!(!registry.is_empty());
    assert!(registry
        .resource_types()
        .iter()
        .any(|n| n.ends_with("Post")));
}

/// Positive: the author's `update` ability is allowed by `PostPolicy`.
#[test]
fn author_can_update_own_post() {
    let registry = registry();
    let post = post("user-1");
    let author = user("user-1");
    assert!(registry.can(Some(&author), &post, "update"));
    assert!(!registry.denies(Some(&author), &post, "update"));
    assert_eq!(registry.authorize(Some(&author), &post, "update"), Ok(()));
}

/// Negative: a non-author is denied with a typed `AuthorizationError`.
#[test]
fn non_author_update_is_denied() {
    let registry = registry();
    let post = post("user-1");
    let other = user("user-2");
    assert!(!registry.can(Some(&other), &post, "update"));
    assert!(registry.denies(Some(&other), &post, "update"));
    assert_eq!(
        registry.authorize(Some(&other), &post, "update"),
        Err(AuthorizationError::Denied {
            ability: "update".to_string()
        })
    );
}

/// An unauthenticated caller never becomes a silent allow.
#[test]
fn unauthenticated_user_fails_closed() {
    let registry = registry();
    let post = post("user-1");
    assert!(!registry.can(None, &post, "update"));
    assert_eq!(
        registry.authorize(None, &post, "update"),
        Err(AuthorizationError::Denied {
            ability: "update".to_string()
        })
    );
}

/// A non-standard ability name maps to no action and is not defined.
#[test]
fn unknown_ability_is_ability_not_defined() {
    let registry = registry();
    let post = post("user-1");
    let author = user("user-1");
    assert_eq!(
        registry.authorize(Some(&author), &post, "publish"),
        Err(AuthorizationError::AbilityNotDefined {
            ability: "publish".to_string()
        })
    );
}

/// A resource type with no registered policy fails closed.
#[test]
fn unregistered_type_fails_closed() {
    struct Comment;
    let registry = registry();
    let author = user("user-1");
    assert_eq!(
        registry.authorize(Some(&author), &Comment, "update"),
        Err(AuthorizationError::AbilityNotDefined {
            ability: "update".to_string()
        })
    );
}

/// Both camelCase and snake_case spellings map to the same action.
#[test]
fn action_names_map_both_spellings() {
    assert_eq!(
        PolicyAction::from_ability("viewAny"),
        Some(PolicyAction::ViewAny)
    );
    assert_eq!(
        PolicyAction::from_ability("view_any"),
        Some(PolicyAction::ViewAny)
    );
    assert_eq!(
        PolicyAction::from_ability("update"),
        Some(PolicyAction::Update)
    );
    assert_eq!(PolicyAction::from_ability("nope"), None);
    assert!(PolicyAction::Update.requires_resource());
    assert!(!PolicyAction::Create.requires_resource());
}

/// An action a policy does not override stays fail-closed.
#[test]
fn default_action_fails_closed() {
    let registry = registry();
    let post = post("user-1");
    let author = user("user-1");
    assert_eq!(
        registry.authorize(Some(&author), &post, "delete"),
        Err(AuthorizationError::Denied {
            ability: "delete".to_string()
        })
    );
}

/// `viewAny`/`create` resolve the policy by type without a record method.
#[test]
fn resource_free_actions_resolve_by_type() {
    let registry = registry();
    let post = post("user-1");
    let author = user("user-1");
    assert!(registry.can(Some(&author), &post, "viewAny"));
    assert!(registry.can(Some(&author), &post, "create"));
    assert!(!registry.can(None, &post, "viewAny"));
}

/// Cloned handles share one registry map.
#[test]
fn cloned_registry_shares_state() {
    let registry = registry();
    let handle = registry.clone();
    assert!(handle.contains::<Post>());
    handle.register::<Post, _>(PostPolicy);
    assert_eq!(registry.len(), 1);
}

/// Installing into a `Gate` bridges ability names onto the policies.
#[test]
fn gate_bridge_dispatches_to_policy() {
    let registry = registry();
    let mut gate = Gate::new();
    registry.install(&mut gate);
    assert!(gate.has("update"));

    let post = post("user-1");
    let author = user("user-1");
    let other = user("user-2");
    assert_eq!(gate.authorize_for(Some(&author), "update", &post), Ok(()));
    assert_eq!(
        gate.authorize_for(Some(&other), "update", &post),
        Err(AuthorizationError::Denied {
            ability: "update".to_string()
        })
    );
}
