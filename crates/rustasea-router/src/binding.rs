//! Implicit model route binding — resolve `{param:field}` selectors into a
//! typed model (ADOPT-016, Laravel `UrlRoutable` parity).
//!
//! A route written `/posts/{post:slug}` carries a *binding selector*: the
//! segment after the colon (`slug`) names a [`ModelBinder`] registered through
//! [`Router::register_binding`]. When a binder is registered, at build time
//! [`crate::Router::try_into_axum_router`] wraps the route in a `from_fn` layer
//! that runs *before* the `#[authorize]` layer and the handler. The binder loads
//! the model from the path value and publishes it into the request extensions as
//! a typed value, so a downstream authorizer or handler can read it. When no
//! binder is registered the selector is metadata-only and the route keeps its
//! plain `:param` path parameter:
//!
//! ```rust,ignore
//! struct PostBinder { pool: DbPool }
//!
//! impl ModelBinder for PostBinder {
//!     fn bind(&self, request: &mut Request, value: &str) -> Result<(), Response> {
//!         let pool = self.pool.clone();
//!         let slug = value.to_string();
//!         let post = futures::executor::block_on(async move {
//!             Post::find_by_slug(&pool, &slug).await
//!         })
//!         .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?
//!         .ok_or_else(|| StatusCode::NOT_FOUND.into_response())?;
//!         request.extensions_mut().insert(post);
//!         Ok(())
//!     }
//! }
//!
//! router.register_binding("slug", Arc::new(PostBinder { pool }));
//! ```
//!
//! # Why a registry of binders
//!
//! Rust has no runtime reflection, so the selector alone cannot name the target
//! model type. Following the [`crate::AuthorizeResource`] precedent, the
//! application registers one binder per selector at boot. Binding is *opt-in*:
//! a selector with a registered binder resolves through that binder, while a
//! selector without one is treated as metadata-only (it still feeds `route:list`
//! and the OpenAPI document) and the route falls back to the plain `:param` path
//! parameter. This preserves the pre-binding contract for every existing route,
//! so adding a new `{param:field}` selector never fails an unrelated build.
//!
//! # Parameter extraction
//!
//! The layer captures the route's axum template (`/posts/:post`) at build time
//! and, at request time, splits both the template and `request.uri().path()`
//! into segments — the segment aligned with `:param` is the value handed to the
//! binder. This avoids the private `UrlParams` extension and needs no
//! `MatchedPath` dependency.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::Request;
use axum::response::Response;

use crate::router::Router;

/// A request-scoped resolver for one model binding selector.
///
/// Implemented by the application and registered with
/// [`Router::register_binding`]. [`ModelBinder::bind`] receives the path value
/// for the selector and returns `Ok(())` after publishing the resolved model
/// into the request extensions, or `Err(response)` (typically `404`) to
/// short-circuit the request before the handler runs.
pub trait ModelBinder: Send + Sync + 'static {
    /// Resolve the model for `value` and publish it into the request extensions.
    ///
    /// The request is `&mut` so the binder can insert the resolved model with
    /// [`Request::extensions_mut`]; the same request (and its extensions) then
    /// flows to the downstream `#[authorize]` layer and the handler, which read
    /// it through `axum::Extension<T>`.
    ///
    /// `Ok(())` lets the request proceed; `Err(response)` is returned verbatim.
    // The `Err` variant is a full `Response`, which axum middleware already
    // passes by value; boxing it would add an allocation on the miss path for
    // no benefit, so the large-`Err` lint is allowed here.
    #[allow(clippy::result_large_err)]
    fn bind(&self, request: &mut Request, value: &str) -> Result<(), Response>;
}

/// Registry mapping binding selectors to their binder.
///
/// Populated by [`Router::register_binding`] and consulted during
/// [`crate::Router::try_into_axum_router`]. A selector declared by a route but
/// absent here is *not* an error: binding is opt-in, so the route falls back to
/// its plain `:param` path parameter (see [`BindingRegistry::resolve`]).
#[derive(Default, Clone)]
pub struct BindingRegistry {
    /// Selector → shared binder.
    binders: HashMap<String, Arc<dyn ModelBinder>>,
}

impl BindingRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert (or replace) the binder registered under `name`.
    pub fn insert(&mut self, name: impl Into<String>, binder: Arc<dyn ModelBinder>) {
        self.binders.insert(name.into(), binder);
    }

    /// Resolve the binder registered under `name`, if any.
    ///
    /// Returns `None` when `name` is unregistered. Binding is opt-in, so the
    /// caller treats a `None` as "no binder for this selector" and falls back to
    /// the plain `:param` path parameter rather than failing the build.
    pub fn resolve(&self, name: &str) -> Option<Arc<dyn ModelBinder>> {
        self.binders.get(name).cloned()
    }

    /// Whether the registry holds no binders.
    pub fn is_empty(&self) -> bool {
        self.binders.is_empty()
    }

    /// Absorb every entry from `other`, replacing same-named binders.
    ///
    /// Used by [`crate::Router::group`] so binders registered inside a group are
    /// available when the parent router is compiled.
    pub fn merge(&mut self, other: BindingRegistry) {
        self.binders.extend(other.binders);
    }
}

impl std::fmt::Debug for BindingRegistry {
    /// Manual debug — the boxed binders have no `Debug` impl.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut names: Vec<&String> = self.binders.keys().collect();
        names.sort();
        formatter
            .debug_struct("BindingRegistry")
            .field("binders", &names)
            .finish()
    }
}

impl Router {
    /// Register a model binder for a `{param:field}` route selector.
    ///
    /// The `binder` captures the concrete model type (typically resolving it
    /// through `Model::find_by_slug`) and is invoked per request. Registration
    /// is keyed by the selector recorded after the colon in a route path.
    /// Binding is opt-in: a route declaring an unregistered selector compiles
    /// unchanged and serves that segment as a plain path parameter.
    ///
    /// ```rust,ignore
    /// router.register_binding("slug", Arc::new(PostBinder { pool }));
    /// router.get_action("/posts/{post:slug}", show_post);
    /// ```
    pub fn register_binding(
        &mut self,
        name: impl Into<String>,
        binder: Arc<dyn ModelBinder>,
    ) -> &mut Self {
        self.binding_registry.insert(name, binder);
        self
    }
}

/// Extract the `(field, param)` selector pairs from a Laravel-style route path.
///
/// Only `{param:field}` segments are selectors (the segment after the colon
/// names the binder); a plain `{param}` is a route parameter with no model
/// binding and is skipped. The scan is a brace walk, mirroring
/// [`crate::route::parse_binding_fields`].
pub(crate) fn selector_pairs(path: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        let tail = &rest[open + 1..];
        let Some(close) = tail.find('}') else {
            break;
        };
        let inner = &tail[..close];
        if let Some((param, field)) = inner.split_once(':') {
            if !param.is_empty() && !field.is_empty() {
                pairs.push((field.to_string(), param.to_string()));
            }
        }
        rest = &tail[close + 1..];
    }
    pairs
}

/// Read the value of `param` from `uri_path` using the axum `template`.
///
/// `template` is the matched route pattern in axum form (`/posts/:post`) and
/// `uri_path` the concrete request path (`/posts/hello-world`). The two are
/// split into segments; the segment aligned with `:param` is returned. A
/// length mismatch, a missing parameter, or a wildcard segment yields `None`.
pub(crate) fn extract_param(uri_path: &str, template: &str, param: &str) -> Option<String> {
    let template_segments: Vec<&str> = template.split('/').collect();
    let uri_segments: Vec<&str> = uri_path.split('/').collect();
    if template_segments.len() != uri_segments.len() {
        return None;
    }
    let target = format!(":{param}");
    template_segments
        .iter()
        .zip(uri_segments.iter())
        .find_map(|(pattern, value)| (*pattern == target).then(|| (*value).to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only `{param:field}` segments are selectors; plain `{param}` is skipped.
    #[test]
    fn selector_pairs_only_reads_colon_forms() {
        assert_eq!(
            selector_pairs("/posts/{post:slug}"),
            vec![("slug".to_string(), "post".to_string())]
        );
        assert!(selector_pairs("/users/{id}").is_empty());
        assert_eq!(
            selector_pairs("/teams/{team}/members/{member:slug}"),
            vec![("slug".to_string(), "member".to_string())]
        );
    }

    /// The template/URI segment walk yields the aligned value.
    #[test]
    fn extract_param_reads_aligned_segment() {
        assert_eq!(
            extract_param("/posts/hello-world", "/posts/:post", "post").as_deref(),
            Some("hello-world")
        );
        assert_eq!(
            extract_param(
                "/teams/7/members/ada",
                "/teams/:team/members/:member",
                "member"
            )
            .as_deref(),
            Some("ada")
        );
        assert!(extract_param("/posts", "/posts/:post", "post").is_none());
    }

    /// Resolving an unregistered selector yields `None` — binding is opt-in.
    #[test]
    fn resolve_unknown_selector_is_none() {
        let registry = BindingRegistry::new();
        assert!(registry.resolve("ghost").is_none());
    }
}
