/// Per-route binding for named limiters.
///
/// The kit binds a named limiter to a route through Laravel's middleware
/// registry. RustaSea's router DSL exposes
/// `Router::register_middleware(name, |MethodRouter| -> MethodRouter)`; this
/// module supplies the missing bridge — a factory that turns a
/// [`RateLimiterRegistry`] + limiter name into a value the router DSL can
/// register.
///
/// # Binding recipe
///
/// The helper returns a plain `Fn(MethodRouter<()>) -> MethodRouter<()>`, which
/// is exactly the shape [`rustasea_router::Router::register_middleware`]
/// accepts. Because `rustasea-auth` must not depend on `rustasea-router`, the
/// caller wires it at the app layer:
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use rustasea_auth::{named_limiter_layer, FortifyLimiterConfig, RateLimiterRegistry};
///
/// let registry = Arc::new(RateLimiterRegistry::from_fortify_config(
///     &FortifyLimiterConfig::default(),
/// ));
/// let table = /* &mut rustasea_router::Router */;
/// table.register_middleware("throttle.login", named_limiter_layer(registry, "login"));
/// // then on the route group: table.middleware("throttle.login");
/// ```
///
/// # Identity source
///
/// A route middleware runs before the handler and cannot read a form body, so
/// the identity fields the limiters key on are read from **request
/// extensions** stamped by an upstream layer (or by the app's own extractor):
///
/// * [`ThrottleUsername`] — the submitted username / email (`login`).
/// * [`ThrottleSessionId`] — the session id, authenticated or pending
///   2FA (`two-factor`, `passkeys`).
/// * [`ThrottleCredentialId`] — the WebAuthn credential id (`passkeys`).
///
/// The peer IP is read from `axum::extract::ConnectInfo<SocketAddr>`, exactly as
/// [`crate::throttle::layer::ThrottleService`] does. Apps that derive the key in
/// the handler instead should call [`RateLimiterRegistry::check`] directly — the
/// registry is the primitive; this module is the middleware sugar.
use std::sync::Arc;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::routing::MethodRouter;

use super::layer::{peer_ip, throttle_response};
use super::registry::{LimiterInput, RateLimiterRegistry};
use super::ThrottleDecision;

/// Request extension carrying the submitted username / email.
///
/// Stamp it before the limiter middleware runs (e.g. from a layer that parses
/// the login form) so the `login` limiter can derive its bucket key.
#[derive(Clone, Debug)]
pub struct ThrottleUsername(pub String);

/// Request extension carrying the session id (authenticated or pending 2FA).
///
/// The `two-factor` limiter keys on this alone; the `passkeys` limiter uses it
/// as a fallback when no credential id is present.
#[derive(Clone, Debug)]
pub struct ThrottleSessionId(pub String);

/// Request extension carrying the WebAuthn credential id.
///
/// The `passkeys` limiter prefers this over [`ThrottleSessionId`].
#[derive(Clone, Debug)]
pub struct ThrottleCredentialId(pub String);

/// Project a request's extensions + a resolved peer address into a borrowed
/// [`LimiterInput`].
///
/// `peer_ip` is passed in rather than read here so the caller owns the
/// `String` for the duration of the borrow (the peer address is a
/// `SocketAddr` formatted to a temporary). Missing extensions yield `None` for
/// that identity field, which the limiter definitions treat as "decline to
/// derive a key" (fail closed). The returned borrow lives as long as `request`
/// and `peer_ip`.
pub fn limiter_input<'a>(request: &'a Request, peer_ip: Option<&'a str>) -> LimiterInput<'a> {
    let username = request.extensions().get::<ThrottleUsername>();
    let session = request.extensions().get::<ThrottleSessionId>();
    let credential = request.extensions().get::<ThrottleCredentialId>();
    LimiterInput {
        username: username.map(|u| u.0.as_str()),
        ip: peer_ip,
        session_id: session.map(|s| s.0.as_str()),
        credential_id: credential.map(|c| c.0.as_str()),
    }
}

/// Build a router middleware transform that enforces the named limiter.
///
/// The returned closure matches the router DSL's
/// `register_middleware` signature (`Fn(MethodRouter<()>) -> MethodRouter<()>`).
/// It layers an `axum::middleware::from_fn` middleware that:
///
/// 1. projects the request into a [`LimiterInput`] via [`limiter_input`];
/// 2. asks the registry for one hit ([`RateLimiterRegistry::check`]);
/// 3. forwards the request on [`ThrottleDecision::Allowed`], or answers `429`
///    with `Retry-After` on [`ThrottleDecision::Denied`] (the same envelope as
///    [`crate::throttle::layer::ThrottleService`]).
///
/// An **unknown limiter name** (a wiring bug) fails closed with `500` — never an
/// allow — so a misconfigured route cannot become unlimited. Register the
/// limiter in the registry, or fix the name.
///
/// The returned value is cheap to clone and shares the registry through an
/// [`Arc`].
pub fn named_limiter_layer(
    registry: Arc<RateLimiterRegistry>,
    name: impl Into<String>,
) -> impl Fn(MethodRouter<()>) -> MethodRouter<()> + Send + Sync + 'static {
    let name = name.into();
    move |method_router: MethodRouter<()>| {
        let registry = Arc::clone(&registry);
        let name = name.clone();
        let middleware = axum::middleware::from_fn(move |request: Request, next: Next| {
            let registry = Arc::clone(&registry);
            let name = name.clone();
            async move {
                let ip = peer_ip(&request);
                let input = limiter_input(&request, ip.as_deref());
                match registry.check(&name, &input) {
                    Ok(ThrottleDecision::Allowed { .. }) => next.run(request).await,
                    Ok(ThrottleDecision::Denied { retry_after_secs }) => {
                        throttle_response(retry_after_secs)
                    }
                    Err(error) => {
                        // Misconfiguration: the route names a limiter that is
                        // not registered. Fail closed (500) rather than
                        // forwarding an unlimited request.
                        let body = format!("rate limiter misconfigured: {error}");
                        (StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
                    }
                }
            }
        });
        method_router.layer(middleware)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::throttle::defaults;
    use crate::throttle::limiter::MemoryRateLimiter;
    use axum::body::Body;
    use axum::routing::get;
    use tower::ServiceExt;

    /// Build a request with a peer address and stamped identity extensions.
    fn request(ip: &str, username: Option<&str>) -> Request {
        let mut req = Request::builder()
            .uri("/login")
            .body(Body::empty())
            .expect("request builds");
        let addr: std::net::SocketAddr = format!("{ip}:54321").parse().expect("addr parses");
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(addr));
        if let Some(username) = username {
            req.extensions_mut()
                .insert(ThrottleUsername(username.to_string()));
        }
        req
    }

    fn registry_with_login() -> Arc<RateLimiterRegistry> {
        let limiter = Arc::new(MemoryRateLimiter::new());
        limiter.set_now(1_000_000);
        let registry = RateLimiterRegistry::new(limiter);
        registry.register("login", defaults::login_definition());
        Arc::new(registry)
    }

    /// The layer allows up to the limit then answers 429 + Retry-After.
    #[tokio::test]
    async fn layer_denies_with_429_after_limit() {
        let layer = named_limiter_layer(registry_with_login(), "login");
        let svc = layer(get(|| async { "ok" }));

        for _ in 0..5 {
            let resp = svc
                .clone()
                .oneshot(request("1.2.3.4", Some("ada@example.com")))
                .await
                .expect("call");
            assert_eq!(resp.status(), StatusCode::OK);
        }
        let denied = svc
            .oneshot(request("1.2.3.4", Some("ada@example.com")))
            .await
            .expect("call");
        assert_eq!(denied.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(denied
            .headers()
            .get(axum::http::header::RETRY_AFTER)
            .is_some());
    }

    /// Case-insensitive usernames share one bucket through the layer.
    #[tokio::test]
    async fn layer_buckets_usernames_case_insensitively() {
        let layer = named_limiter_layer(registry_with_login(), "login");
        let svc = layer(get(|| async { "ok" }));
        for _ in 0..5 {
            let resp = svc
                .clone()
                .oneshot(request("1.2.3.4", Some("ADA@Example.com")))
                .await
                .expect("call");
            assert_eq!(resp.status(), StatusCode::OK);
        }
        let denied = svc
            .oneshot(request("1.2.3.4", Some("ada@example.com")))
            .await
            .expect("call");
        assert_eq!(denied.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    /// An unknown limiter name fails closed with 500, not an allow.
    #[tokio::test]
    async fn unknown_limiter_fails_closed() {
        let layer = named_limiter_layer(registry_with_login(), "missing");
        let svc = layer(get(|| async { "ok" }));
        let resp = svc
            .oneshot(request("1.2.3.4", Some("ada@example.com")))
            .await
            .expect("call");
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
