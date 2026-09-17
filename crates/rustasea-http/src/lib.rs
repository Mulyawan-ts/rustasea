//! RustaSea HTTP layer — AppState, JSON helpers, middleware stubs, and HTTP client.

/// Application error type + dev/prod error renderers (ADOPT-010).
pub mod error;

/// Panic catching + dev panic-location capture (ADOPT-010).
pub mod panic;

/// Idle (inter-chunk) timeout enforcement for buffered client responses.
mod idle;

/// Sentry request-context middleware (ADOPT-004) — opt-in via the `sentry`
/// feature; a no-op while no Sentry client is bound.
#[cfg(feature = "sentry")]
pub mod sentry;

use std::any::Any;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json as AxumJson;
use serde::Serialize;
use serde_json::Value;

/// Security posture consumed by HTTP middleware.
///
/// Plain data only — the enforcing types (PreventRequestForgery, Throttle)
/// live in `rustasea-auth`, which depends on this crate; keeping the config
/// data here preserves the DAG (AUTH -> HTTP) without cycles.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SecurityConfig {
    /// Origin allow-list for CSRF (`config.app.csrf_origins`).
    pub csrf_origins: Vec<String>,
    /// Trusted proxy CIDRs; `X-Forwarded-For` is ignored until non-empty.
    pub trusted_proxies: Vec<String>,
}

impl SecurityConfig {
    /// Create a security config from a CSRF origin allow-list.
    pub fn with_csrf_origins(origins: Vec<String>) -> Self {
        Self {
            csrf_origins: origins,
            trusted_proxies: Vec::new(),
        }
    }

    /// Mark proxies as trusted so forwarded identities are honored.
    pub fn behind_proxies(mut self, proxies: Vec<String>) -> Self {
        self.trusted_proxies = proxies;
        self
    }
}

/// Shared application state passed to handlers.
///
/// # Auth seam (ADR-0007 alignment)
///
/// The auth subsystem (`rustasea_auth::SessionGuard`) cannot be named here:
/// `rustasea-auth` depends on `rustasea-http` (it consumes [`SecurityConfig`]
/// and this type), so naming the guard would create a dependency cycle. The
/// auth slot is therefore **type-erased** — an `Option<Arc<dyn Any + Send + Sync>>`
/// installed with [`AppState::with_auth`] and read back with the typed accessor
/// [`AppState::auth`]. `rustasea-app` stores its concrete guard and its
/// middleware downcasts it back, keeping the DAG acyclic.
///
/// This is the object-erased variant of the "small object-safe trait" seam: it
/// needs no async trait object, no boxed futures, and no new dependency, while
/// still letting the app install and resolve any `Send + Sync + 'static` value.
///
/// # Fail-closed
///
/// The slot defaults to `None` and [`AppState::auth`] returns a typed `None`
/// when it is absent **or** holds a different concrete type. Nothing here ever
/// panics, so an un-wired app simply never authenticates and the existing gates
/// redirect.
#[derive(Clone)]
pub struct AppState {
    /// Application environment name.
    pub env: String,
    /// Debug flag.
    pub debug: bool,
    /// Security posture (CSRF origins, trusted proxies).
    pub security: SecurityConfig,
    /// Type-erased authentication backend (see the type-level docs).
    auth: Option<Arc<dyn Any + Send + Sync>>,
}

impl AppState {
    /// Create a new AppState with no authentication backend wired.
    pub fn new(env: impl Into<String>, debug: bool) -> Self {
        Self {
            env: env.into(),
            debug,
            security: SecurityConfig::default(),
            auth: None,
        }
    }

    /// Attach the security posture.
    pub fn with_security(mut self, security: SecurityConfig) -> Self {
        self.security = security;
        self
    }

    /// Install the authentication backend (type-erased).
    ///
    /// The value is typically the app's `SessionGuard` wrapped in an [`Arc`],
    /// but any `Send + Sync + 'static` type is accepted so the HTTP layer stays
    /// auth-agnostic. Reading it back is an [`AppState::auth`] downcast.
    pub fn with_auth<T>(mut self, auth: Arc<T>) -> Self
    where
        T: Any + Send + Sync,
    {
        self.auth = Some(auth);
        self
    }

    /// Resolve the authentication backend as its concrete type `T`.
    ///
    /// Returns `None` — never a panic — when no backend is installed or the
    /// installed backend is not a `T`. Callers must treat `None` as
    /// "unauthenticated" and fail closed.
    pub fn auth<T>(&self) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
    {
        self.auth
            .as_ref()
            .and_then(|backend| Arc::clone(backend).downcast::<T>().ok())
    }

    /// Whether an authentication backend is installed.
    pub fn has_auth(&self) -> bool {
        self.auth.is_some()
    }

    /// Convenience accessor for the CSRF origin allow-list.
    pub fn csrf_origins(&self) -> &[String] {
        &self.security.csrf_origins
    }
}

impl std::fmt::Debug for AppState {
    /// Manual debug — the erased backend is rendered as a presence flag, never
    /// by contents (it holds no `Debug` bound).
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppState")
            .field("env", &self.env)
            .field("debug", &self.debug)
            .field("security", &self.security)
            .field("auth", &self.has_auth())
            .finish()
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new("production", false)
    }
}

/// Helper to build JSON responses with status code.
pub struct JsonResponse;

impl JsonResponse {
    /// Build a 200 JSON response from a serializable value.
    pub fn ok<T: Serialize>(data: T) -> Response {
        (StatusCode::OK, AxumJson(data)).into_response()
    }

    /// Build a JSON response with custom status.
    pub fn with_status<T: Serialize>(status: StatusCode, data: T) -> Response {
        (status, AxumJson(data)).into_response()
    }

    /// Build a 422 validation error response.
    pub fn validation_error(errors: Value) -> Response {
        (StatusCode::UNPROCESSABLE_ENTITY, AxumJson(errors)).into_response()
    }
}

/// CORS middleware configuration stub.
#[derive(Debug, Clone)]
pub struct CorsConfig {
    /// Allowed origins.
    pub allowed_origins: Vec<String>,
    /// Allow credentials.
    pub allow_credentials: bool,
}

impl CorsConfig {
    /// Create CORS config with allowed origins.
    pub fn new(origins: Vec<String>) -> Self {
        Self {
            allowed_origins: origins,
            allow_credentials: false,
        }
    }

    /// Build the tower-http CORS layer.
    ///
    /// The layer composes onto any tower/axum stack, so routers apply it
    /// directly:
    ///
    /// ```rust,ignore
    /// use rustasea_http::CorsConfig;
    /// let layer = CorsConfig::default().layer();
    /// // router.layer(layer); — rustasea::router::Router::layer()
    /// ```
    ///
    /// Allowed origins map to an explicit AllowOrigin::list; when none are
    /// configured the layer stays restrictive (no origins allowed) instead of
    /// silently becoming permissive — opt into permissive explicitly.
    pub fn layer(&self) -> tower_http::cors::CorsLayer {
        use tower_http::cors::AllowOrigin;
        use tower_http::cors::CorsLayer;

        let mut layer = CorsLayer::new();
        if !self.allowed_origins.is_empty() {
            let origins: Vec<_> = self
                .allowed_origins
                .iter()
                .map(|o| o.parse::<axum::http::HeaderValue>())
                .filter_map(Result::ok)
                .collect();
            layer = layer.allow_origin(AllowOrigin::list(origins));
        }
        if self.allow_credentials {
            layer = layer.allow_credentials(true);
        }
        layer
    }
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

/// Throttle / rate-limit middleware configuration.
///
/// The enforcing `MemoryRateLimiter` + tower middleware live in
/// `rustasea-auth` (M3). This config type is the HTTP-layer representation
/// parsed from `#[middleware("throttle:60,1")]` specs before delegation.
#[derive(Debug, Clone)]
pub struct ThrottleConfig {
    /// Max requests per window.
    pub max_requests: u32,
    /// Window duration in seconds.
    pub window_secs: u64,
    /// Key bucket: by_ip or by_user.
    pub bucket: String,
}

impl ThrottleConfig {
    /// Create a new throttle config.
    pub fn new(max_requests: u32, window_secs: u64) -> Self {
        Self {
            max_requests,
            window_secs,
            bucket: "by_ip".to_string(),
        }
    }

    /// Set bucket key to by_ip.
    pub fn by_ip(mut self) -> Self {
        self.bucket = "by_ip".to_string();
        self
    }

    /// Set bucket key to by_user.
    pub fn by_user(mut self) -> Self {
        self.bucket = "by_user".to_string();
        self
    }
}

/// Timeout classification for the HTTP client (#18).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutKind {
    /// Connection establishment exceeded the timeout.
    Connect,
    /// Total request time exceeded the timeout.
    Total,
    /// No bytes received within the idle timeout.
    Idle,
}

/// Typed error returned by [`HttpClient::send`].
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// The `throw` predicate matched the response status.
    #[error("upstream {code} matched throw predicate")]
    Status { code: u16 },
    /// A request timeout fired; kind distinguishes connect/total/idle.
    #[error("http client timed out ({kind:?})")]
    Timeout { kind: TimeoutKind },
    /// The `throw` predicate itself failed.
    #[error("throw callback failed: {source}")]
    ThrowCallback {
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// Transport-level failure surfaced by reqwest.
    #[error("http transport error: {source}")]
    Transport { source: reqwest::Error },
}

/// Result of evaluating a `throw` predicate; an `Err` aborts the send as
/// [`HttpError::ThrowCallback`].
type ThrowOutcome = Result<bool, Box<dyn std::error::Error + Send + Sync>>;

/// Shared `throw` callback boxed signature.
type ThrowCallback = dyn Fn(&reqwest::Response) -> ThrowOutcome + Send + Sync;

/// Thin wrapper around reqwest for outgoing HTTP calls.
///
/// Laravel-style builder: `.timeout(Duration)` configures the total request
/// budget, `.throw(predicate)` maps a matching status to
/// [`HttpError::Status`], and `.send().await` returns
/// `Result<reqwest::Response, HttpError>`.
pub struct HttpClient {
    /// Per-request timeout budget.
    pub timeout: std::time::Duration,
    /// Optional idle timeout (inter-byte silence).
    pub idle_timeout: Option<std::time::Duration>,
    /// Optional `throw` predicate evaluated against the response.
    throw: Option<Box<ThrowCallback>>,
}

impl HttpClient {
    /// Create a new HTTP client with a 30s default timeout.
    pub fn new() -> Self {
        Self {
            timeout: std::time::Duration::from_secs(30),
            idle_timeout: None,
            throw: None,
        }
    }

    /// Set the total request timeout.
    ///
    /// ```rust
    /// # use std::time::Duration;
    /// use rustasea_http::HttpClient;
    /// let client = HttpClient::new().timeout(Duration::from_secs(30));
    /// # let _ = client;
    /// ```
    pub fn timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set the idle (inter-chunk) timeout.
    ///
    /// When set, the response body is read chunk by chunk in
    /// [`send`](Self::send) / [`send_with`](Self::send_with) and each gap
    /// between consecutive chunks is bounded by this budget. A stall longer
    /// than the budget aborts the send with [`HttpError::Timeout`] carrying
    /// [`TimeoutKind::Idle`]. The timer resets on every received chunk, so a
    /// slow but steady stream is not penalised. When unset the response is
    /// returned untouched with zero added overhead.
    ///
    /// ```rust
    /// # use std::time::Duration;
    /// use rustasea_http::HttpClient;
    /// let client = HttpClient::new().idle_timeout(Duration::from_secs(5));
    /// # let _ = client;
    /// ```
    pub fn idle_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.idle_timeout = Some(timeout);
        self
    }

    /// Install a `throw` predicate: when it returns `true` for the response
    /// status, [`send`](Self::send) yields [`HttpError::Status`].
    ///
    /// ```rust
    /// # use std::time::Duration;
    /// use rustasea_http::HttpClient;
    /// let client = HttpClient::new()
    ///     .timeout(Duration::from_secs(30))
    ///     .throw(|resp| resp.status().is_success());
    /// # let _ = client;
    /// ```
    pub fn throw<F>(mut self, predicate: F) -> Self
    where
        F: Fn(&reqwest::Response) -> bool + Send + Sync + 'static,
    {
        self.throw = Some(Box::new(move |resp| Ok(predicate(resp))));
        self
    }

    /// Install a fallible `throw` callback returning `Result<bool, E>`.
    ///
    /// An `Err` from the callback surfaces as [`HttpError::ThrowCallback`];
    /// an `Ok(true)` match surfaces as [`HttpError::Status`].
    pub fn try_throw<F, E>(mut self, callback: F) -> Self
    where
        F: Fn(&reqwest::Response) -> Result<bool, E> + Send + Sync + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        self.throw = Some(Box::new(move |resp| {
            callback(resp).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        }));
        self
    }

    /// Send a prepared reqwest request through this client's policy.
    ///
    /// Applies the configured total timeout, then evaluates the `throw`
    /// predicate: a `true` match becomes [`HttpError::Status`]. When an idle
    /// timeout is configured the response body is additionally drained under
    /// [`TimeoutKind::Idle`] enforcement.
    pub async fn send(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, HttpError> {
        self.send_with(request, reqwest::Client::new()).await
    }

    /// Send a prepared request through an explicit reqwest client.
    pub async fn send_with(
        &self,
        request: reqwest::RequestBuilder,
        client: reqwest::Client,
    ) -> Result<reqwest::Response, HttpError> {
        // reqwest needs a tokio runtime; building per-send keeps the wrapper
        // clone-free. Map request-build errors straight to Transport.
        let request = request
            .timeout(self.timeout)
            .build()
            .map_err(|e| HttpError::Transport { source: e })?;
        let response = client.execute(request).await.map_err(map_timeout)?;
        if let Some(throw) = &self.throw {
            let matched = throw(&response).map_err(|source| HttpError::ThrowCallback { source })?;
            if matched {
                return Err(HttpError::Status {
                    code: response.status().as_u16(),
                });
            }
        }
        // Enforce the idle (inter-chunk) timeout by draining the body when a
        // budget is configured. The plain path returns the response untouched
        // so callers that never set an idle timeout pay nothing.
        match self.idle_timeout {
            Some(idle) => idle::buffer_with_idle_timeout(response, idle).await,
            None => Ok(response),
        }
    }

    /// Convenience: perform a GET request with this client's policy.
    pub async fn get(&self, url: &str) -> Result<reqwest::Response, HttpError> {
        self.send(reqwest::Client::new().get(url)).await
    }

    /// Convenience: perform a POST request with this client's policy.
    pub async fn post(&self, url: &str) -> Result<reqwest::Response, HttpError> {
        self.send(reqwest::Client::new().post(url)).await
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Classify a reqwest transport error into a typed error.
///
/// Connection-phase failures map to [`TimeoutKind::Connect`] and total-budget
/// expiry to [`TimeoutKind::Total`]. Idle (inter-chunk) timeout enforcement
/// lives in [`crate::idle`]: the body is drained under a per-chunk deadline
/// and a stall surfaces as [`HttpError::Timeout`] with [`TimeoutKind::Idle`].
pub(crate) fn map_timeout(error: reqwest::Error) -> HttpError {
    if error.is_connect() {
        HttpError::Timeout {
            kind: TimeoutKind::Connect,
        }
    } else if error.is_timeout() {
        HttpError::Timeout {
            kind: TimeoutKind::Total,
        }
    } else {
        HttpError::Transport { source: error }
    }
}
