//! `#[authorize]` runtime bridge — Gate evaluation over request extensions.
//!
//! The `rustasea-router` authorization layer ([`rustasea_router::AuthorizeResource`])
//! knows *when* to run an ability check but not *how*: Rust has no reflection, so
//! it cannot discover the target model's type from the `#[authorize]` attribute
//! alone. This module supplies the missing half — [`GateResource<T>`] binds a
//! concrete resource type `T` to a [`Gate`] and, at request time, reads the
//! principal and the resolved model out of the request extensions and calls
//! [`Gate::authorize_for`].
//!
//! # Wiring
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use rustasea_auth::{Gate, GateResource};
//!
//! let gate = Arc::new(build_gate()); // abilities defined + policies installed
//! router.register_authorizer("post", Arc::new(GateResource::<Post>::new(gate)));
//! router.authorize_meta(__RUSTASEA_AUTHORIZE_update_post)
//!       .put_action("/posts/{id}", update_post);
//! ```
//!
//! # The resource contract
//!
//! A binding middleware (or the handler's own pre-step) must insert the resolved
//! model into the request extensions as the concrete type `T`:
//!
//! ```rust,ignore
//! request.extensions_mut().insert(post); // post: Post
//! ```
//!
//! The auth middleware installed by the application (see the app's
//! `with_session` layer) inserts the [`AuthUser`] principal the same way. When
//! either is absent the check fails closed with a `403` — a missing principal is
//! an unauthenticated caller and a missing resource is a wiring gap, and neither
//! may widen access.
//!
//! # Why not the Gate's sync user resolver
//!
//! [`Gate::with_user_resolver`] installs a `Fn() -> Option<AuthUser>` closure
//! with no request context, so it cannot read the per-request extensions. This
//! bridge therefore takes the *explicit-user* entry point
//! ([`Gate::authorize_for`]) and threads the principal it reads from the request
//! — the same `AuthUser` an auth middleware resolved — directly into the Gate.
//! This keeps the Gate free of request-scoped state and composes with the
//! router's innermost authorization layer.
use std::marker::PhantomData;
use std::sync::Arc;

use axum::extract::Request;
use axum::response::{IntoResponse, Response};
use rustasea_router::AuthorizeResource;

use crate::gate::{AuthorizationError, Gate};
use crate::guard::AuthUser;

/// Bridges a [`Gate`] to the router's `#[authorize]` layer for resource type `T`.
///
/// The `PhantomData<fn() -> T>` marker records `T` without owning a value, so the
/// adapter is `Send + Sync` whenever the Gate is and `T` needs no `Clone`/
/// `Default` bound. Construct one per resource id and register it with
/// `Router::register_authorizer`.
pub struct GateResource<T> {
    gate: Arc<Gate>,
    _marker: PhantomData<fn() -> T>,
}

impl<T> GateResource<T> {
    /// Bind `gate` to resource type `T`.
    pub fn new(gate: Arc<Gate>) -> Self {
        Self {
            gate,
            _marker: PhantomData,
        }
    }
}

impl<T> AuthorizeResource for GateResource<T>
where
    T: Send + Sync + 'static,
{
    /// Authorize `ability` for the request's principal against its `T` resource.
    ///
    /// Reads the [`AuthUser`] and the `T` resource from the request extensions
    /// and delegates to [`Gate::authorize_for`]. A missing principal or resource
    /// is denied; a Gate denial renders the shared `403` JSON envelope.
    fn authorize(&self, request: &Request, ability: &str) -> Result<(), Response> {
        let user = request.extensions().get::<AuthUser>();
        let Some(resource) = request.extensions().get::<T>() else {
            // No resolved resource — a wiring gap must never widen access.
            return Err(AuthorizationError::Denied {
                ability: ability.to_string(),
            }
            .into_response());
        };
        self.gate
            .authorize_for(user, ability, resource)
            .map_err(IntoResponse::into_response)
    }
}

impl<T> std::fmt::Debug for GateResource<T> {
    /// Manual debug — the Gate has a `Debug` impl but `T` is a marker only.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GateResource")
            .field("resource", &std::any::type_name::<T>())
            .finish()
    }
}

/// Build a type-erased [`GateResource`] for `T`, ready for registration.
///
/// Ergonomic shorthand for `Arc::new(GateResource::<T>::new(gate))`:
///
/// ```rust,ignore
/// router.register_authorizer("post", rustasea_auth::authorizer_for::<Post>(gate));
/// ```
pub fn authorizer_for<T>(gate: Arc<Gate>) -> Arc<dyn AuthorizeResource>
where
    T: Send + Sync + 'static,
{
    Arc::new(GateResource::<T>::new(gate))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A blog post owned by one author.
    #[derive(Clone)]
    struct Post {
        author_id: String,
    }

    fn user(id: &str) -> AuthUser {
        AuthUser::new(id, Some(format!("{id}@example.com")), "session")
    }

    /// Gate where only the author may `update` a post.
    fn post_gate() -> Arc<Gate> {
        let mut gate = Gate::new();
        gate.define("update", |user, target| {
            let Some(post) = target.downcast_ref::<Post>() else {
                return false;
            };
            user.map(|u| u.id == post.author_id).unwrap_or(false)
        });
        Arc::new(gate)
    }

    /// Build a request carrying an optional principal and post.
    fn request(user: Option<AuthUser>, post: Option<Post>) -> Request {
        let builder = Request::builder().uri("/posts/1");
        let mut request = builder.body(axum::body::Body::empty()).expect("request");
        if let Some(user) = user {
            request.extensions_mut().insert(user);
        }
        if let Some(post) = post {
            request.extensions_mut().insert(post);
        }
        request
    }

    /// Positive: the owner's request passes the Gate and is allowed.
    #[test]
    fn owner_is_allowed() {
        let resource = GateResource::<Post>::new(post_gate());
        let request = request(
            Some(user("user-1")),
            Some(Post {
                author_id: "user-1".to_string(),
            }),
        );
        assert!(resource.authorize(&request, "update").is_ok());
    }

    /// Negative: a non-owner is denied with the `403` envelope.
    #[test]
    fn non_owner_is_forbidden() {
        let resource = GateResource::<Post>::new(post_gate());
        let request = request(
            Some(user("user-2")),
            Some(Post {
                author_id: "user-1".to_string(),
            }),
        );
        let response = resource
            .authorize(&request, "update")
            .expect_err("non-owner must be denied");
        assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
    }

    /// A missing principal fails closed (unauthenticated caller).
    #[test]
    fn missing_user_fails_closed() {
        let resource = GateResource::<Post>::new(post_gate());
        let request = request(
            None,
            Some(Post {
                author_id: "user-1".to_string(),
            }),
        );
        let response = resource
            .authorize(&request, "update")
            .expect_err("missing principal must deny");
        assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
    }

    /// A missing resolved resource fails closed (wiring gap).
    #[test]
    fn missing_resource_fails_closed() {
        let resource = GateResource::<Post>::new(post_gate());
        let request = request(Some(user("user-1")), None);
        let response = resource
            .authorize(&request, "update")
            .expect_err("missing resource must deny");
        assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
    }
}
