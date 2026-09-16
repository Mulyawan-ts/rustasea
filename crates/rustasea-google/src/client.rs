//! Service-account auth client with a single-flight access-token cache.
//!
//! [`GoogleAuthClient`] signs an RS256 assertion ([`crate::claims`]), exchanges
//! it at the account's token endpoint over a swappable [`TokenTransport`], and
//! caches the resulting [`AccessToken`]. Concurrent callers share one refresh:
//! the cache mutex is held across the exchange, so a burst of `token()` calls
//! performs exactly one HTTP round-trip. A token is renewed once 80% of its
//! lifetime has elapsed ([`DEFAULT_REFRESH_RATIO`]).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::claims::AssertionClaims;
use crate::credentials::ServiceAccount;
use crate::error::{GoogleAuthError, Result};
use crate::token::AccessToken;

/// Default per-request timeout for the token endpoint, in seconds.
pub const DEFAULT_TIMEOUT_SECS: u64 = 10;

/// Fraction of a token's lifetime after which the cache renews it.
pub const DEFAULT_REFRESH_RATIO: f64 = 0.8;

/// The `jwt-bearer` grant type used for service-account assertions.
pub const JWT_BEARER_GRANT: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

/// Wall-clock abstraction, so the refresh schedule is testable.
pub trait Clock: Send + Sync + 'static {
    /// The current instant in UTC.
    fn now(&self) -> DateTime<Utc>;
}

/// The real system clock.
pub struct SystemClock;

impl Clock for SystemClock {
    /// Return the current UTC instant.
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// An outbound `application/x-www-form-urlencoded` POST to the token endpoint.
///
/// Returns `(status, body)` rather than a decoded value so the client owns the
/// status-code policy. The production [`ReqwestTransport`] performs the HTTPS
/// call; tests install a recording mock.
#[async_trait]
pub trait TokenTransport: Send + Sync + 'static {
    /// POST the URL-encoded `fields` to `url`, returning the status and body.
    ///
    /// # Errors
    ///
    /// [`GoogleAuthError::Transport`] on a transport-level failure.
    async fn post_form(&self, url: &str, fields: Vec<(String, String)>) -> Result<(u16, String)>;
}

/// `reqwest`-backed transport with a per-request timeout.
pub struct ReqwestTransport {
    /// Configured HTTP client.
    client: reqwest::Client,
}

impl ReqwestTransport {
    /// Build a transport with the given request timeout.
    ///
    /// A client-build failure falls back to `reqwest::Client::new()` so a
    /// misconfigured TLS backend cannot panic the process.
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client }
    }
}

#[async_trait]
impl TokenTransport for ReqwestTransport {
    /// POST the URL-encoded fields and return `(status, body_text)`.
    async fn post_form(&self, url: &str, fields: Vec<(String, String)>) -> Result<(u16, String)> {
        let response = self
            .client
            .post(url)
            .form(&fields)
            .send()
            .await
            .map_err(|error| GoogleAuthError::Transport {
                message: error.to_string(),
            })?;
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        Ok((status, text))
    }
}

/// A source of Google access tokens — the seam consumers depend on.
#[async_trait]
pub trait GoogleTokenSource: Send + Sync + 'static {
    /// Return a cached (or freshly exchanged) access token.
    ///
    /// # Errors
    ///
    /// Any [`GoogleAuthError`] raised while signing or exchanging an assertion.
    async fn token(&self) -> Result<Arc<AccessToken>>;
}

/// Service-account auth client with an in-process token cache.
pub struct GoogleAuthClient {
    /// Parsed service-account credentials.
    account: ServiceAccount,
    /// OAuth scopes requested in the assertion.
    scopes: Vec<String>,
    /// Impersonated subject for domain-wide delegation, when set.
    subject: Option<String>,
    /// Token-endpoint transport.
    transport: Arc<dyn TokenTransport>,
    /// Wall clock used for expiry decisions.
    clock: Arc<dyn Clock>,
    /// Cached token, guarded for single-flight refresh.
    cache: Mutex<Option<Arc<AccessToken>>>,
    /// Lifetime fraction after which the cache renews.
    refresh_ratio: f64,
}

impl GoogleAuthClient {
    /// Build a client for a service account.
    ///
    /// # Errors
    ///
    /// [`GoogleAuthError::MissingScopes`] when `scopes` is empty or contains
    /// only blank entries.
    pub fn new(
        account: ServiceAccount,
        scopes: Vec<String>,
        subject: Option<String>,
    ) -> Result<Self> {
        let scopes = clean_scopes(scopes);
        if scopes.is_empty() {
            return Err(GoogleAuthError::MissingScopes);
        }
        Ok(Self {
            account,
            scopes,
            subject,
            transport: Arc::new(ReqwestTransport::new(Duration::from_secs(
                DEFAULT_TIMEOUT_SECS,
            ))),
            clock: Arc::new(SystemClock),
            cache: Mutex::new(None),
            refresh_ratio: DEFAULT_REFRESH_RATIO,
        })
    }

    /// Replace the token transport (tests install a mock).
    #[must_use]
    pub fn with_transport(mut self, transport: Arc<dyn TokenTransport>) -> Self {
        self.transport = transport;
        self
    }

    /// Replace the wall clock (tests install a manual clock).
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Override the refresh ratio; out-of-range values are ignored.
    #[must_use]
    pub fn with_refresh_ratio(mut self, ratio: f64) -> Self {
        if ratio > 0.0 && ratio < 1.0 {
            self.refresh_ratio = ratio;
        }
        self
    }

    /// The service-account credentials (private key masked by `Debug`).
    #[must_use]
    pub fn account(&self) -> &ServiceAccount {
        &self.account
    }

    /// The configured OAuth scopes.
    #[must_use]
    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }

    /// The configured impersonation subject, when any.
    #[must_use]
    pub fn subject(&self) -> Option<&str> {
        self.subject.as_deref()
    }

    /// Return a cached (or freshly exchanged) access token.
    ///
    /// The cache lock is held across the exchange so concurrent callers share
    /// a single refresh (single-flight).
    ///
    /// # Errors
    ///
    /// Any [`GoogleAuthError`] raised while signing or exchanging an assertion.
    pub async fn token(&self) -> Result<Arc<AccessToken>> {
        self.get_token().await
    }

    /// Single-flight cache lookup + refresh.
    async fn get_token(&self) -> Result<Arc<AccessToken>> {
        let mut cache = self.cache.lock().await;
        let now = self.clock.now();
        if let Some(token) = cache.as_ref() {
            if !token.needs_refresh(now, self.refresh_ratio) {
                return Ok(Arc::clone(token));
            }
        }
        let fresh = self.fetch_token(now).await?;
        *cache = Some(Arc::clone(&fresh));
        Ok(fresh)
    }

    /// Sign an assertion and exchange it for a fresh token.
    async fn fetch_token(&self, now: DateTime<Utc>) -> Result<Arc<AccessToken>> {
        let claims = AssertionClaims::for_service_account(
            &self.account,
            &self.scopes,
            self.subject.as_deref(),
            now,
        );
        let assertion = claims.sign(self.account.private_key())?;
        let fields = vec![
            ("grant_type".to_string(), JWT_BEARER_GRANT.to_string()),
            ("assertion".to_string(), assertion),
        ];
        let (status, body) = self
            .transport
            .post_form(self.account.token_uri(), fields)
            .await?;
        if !(200..300).contains(&status) {
            return Err(GoogleAuthError::TokenEndpoint {
                status,
                message: describe_error(&body),
            });
        }
        let response: TokenResponse =
            serde_json::from_str(&body).map_err(|error| GoogleAuthError::TokenResponse {
                message: error.to_string(),
            })?;
        if response.access_token.trim().is_empty() {
            return Err(GoogleAuthError::TokenResponse {
                message: "token endpoint returned an empty access token".to_string(),
            });
        }
        let scopes = response
            .scope
            .as_deref()
            .map(parse_scopes)
            .filter(|scopes| !scopes.is_empty())
            .unwrap_or_else(|| self.scopes.clone());
        let issued_at = now;
        let expires_at = now + chrono::Duration::seconds(response.expires_in.max(0));
        tracing::debug!(
            target: "rustasea_google",
            expires_in = response.expires_in,
            "exchanged service-account assertion for a google access token"
        );
        Ok(Arc::new(AccessToken::new(
            response.access_token,
            response.token_type,
            scopes,
            issued_at,
            expires_at,
        )))
    }
}

impl fmt::Debug for GoogleAuthClient {
    /// Render the client without its cached token or private key.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GoogleAuthClient")
            .field("account", &self.account)
            .field("scopes", &self.scopes)
            .field("subject", &self.subject)
            .field("refresh_ratio", &self.refresh_ratio)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl GoogleTokenSource for GoogleAuthClient {
    /// Delegate to the inherent `token` method.
    async fn token(&self) -> Result<Arc<AccessToken>> {
        self.get_token().await
    }
}

/// Successful token-endpoint response body.
#[derive(Deserialize)]
struct TokenResponse {
    /// Bearer token value.
    access_token: String,
    /// Token type; defaults to `Bearer` when absent.
    #[serde(default = "default_token_type")]
    token_type: String,
    /// Lifetime in seconds.
    expires_in: i64,
    /// Space-separated granted scopes, when reported.
    #[serde(default)]
    scope: Option<String>,
}

/// Error body shape returned by the token endpoint.
#[derive(Deserialize)]
struct OAuthErrorBody {
    /// OAuth error code (e.g. `invalid_grant`).
    error: String,
    /// Human-readable error detail.
    #[serde(default)]
    error_description: Option<String>,
}

/// Default token type when the endpoint omits one.
fn default_token_type() -> String {
    "Bearer".to_string()
}

/// Split a space-separated scope string into owned scopes.
fn parse_scopes(scope: &str) -> Vec<String> {
    scope.split_whitespace().map(str::to_string).collect()
}

/// Drop blank scopes and trim the rest.
fn clean_scopes(scopes: Vec<String>) -> Vec<String> {
    scopes
        .into_iter()
        .map(|scope| scope.trim().to_string())
        .filter(|scope| !scope.is_empty())
        .collect()
}

/// Describe a token-endpoint failure without echoing the raw response body.
fn describe_error(body: &str) -> String {
    match serde_json::from_str::<OAuthErrorBody>(body) {
        Ok(parsed) => match parsed.error_description {
            Some(description) if !description.trim().is_empty() => {
                format!("{} ({})", parsed.error, truncate(&description, 200))
            }
            _ => parsed.error,
        },
        Err(_) => "token exchange rejected; no error detail".to_string(),
    }
}

/// Truncate `text` to at most `max` characters (char-safe).
fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[cfg(test)]
mod tests;
