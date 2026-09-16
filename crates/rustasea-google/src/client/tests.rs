//! Client tests: mocked token exchange, cache reuse, single-flight refresh,
//! impersonation, and error redaction (ADOPT-026). All tests use a recording
//! mock transport and a shared generated RSA key pair, so they are hermetic.

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};

use super::{Clock, GoogleAuthClient, GoogleTokenSource, TokenTransport};
use crate::credentials::ServiceAccount;
use crate::error::{GoogleAuthError, Result};
use crate::test_support::{decode_assertion, key_pair, service_account_json};

/// OAuth scope used across the client tests.
const SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

/// A recorded `(url, form fields)` request.
type RecordedRequest = (String, Vec<(String, String)>);

/// A transport that records requests and returns a canned status/body.
struct MockTransport {
    /// Status code to return.
    status: u16,
    /// Body to return.
    body: String,
    /// Delay before responding, so concurrent callers genuinely overlap.
    delay: Duration,
    /// Number of calls received.
    calls: AtomicUsize,
    /// Recorded requests.
    requests: Mutex<Vec<RecordedRequest>>,
}

impl MockTransport {
    /// A mock returning `200 OK` with `body`.
    fn ok(body: &str) -> Self {
        Self::returning(200, body)
    }

    /// A mock returning `status` with `body`.
    fn returning(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.to_string(),
            delay: Duration::ZERO,
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// Set a response delay.
    #[must_use]
    fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Number of calls received.
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// The most recent recorded request.
    fn last(&self) -> Option<RecordedRequest> {
        self.requests.lock().ok()?.last().cloned()
    }
}

#[async_trait]
impl TokenTransport for MockTransport {
    /// Record the request and return the canned response.
    async fn post_form(&self, url: &str, fields: Vec<(String, String)>) -> Result<(u16, String)> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut guard) = self.requests.lock() {
            guard.push((url.to_string(), fields));
        }
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        Ok((self.status, self.body.clone()))
    }
}

/// A manually advanced clock for deterministic expiry tests.
struct ManualClock {
    /// Base instant.
    base: DateTime<Utc>,
    /// Seconds advanced since construction.
    offset: AtomicI64,
}

impl ManualClock {
    /// Start at `base`.
    fn new(base: DateTime<Utc>) -> Self {
        Self {
            base,
            offset: AtomicI64::new(0),
        }
    }

    /// Advance the clock by `seconds`.
    fn advance(&self, seconds: i64) {
        self.offset.fetch_add(seconds, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    /// Return `base` plus the accumulated offset.
    fn now(&self) -> DateTime<Utc> {
        self.base + ChronoDuration::seconds(self.offset.load(Ordering::SeqCst))
    }
}

/// Parse the shared test service account.
fn service_account() -> ServiceAccount {
    let key = key_pair();
    ServiceAccount::from_json(&service_account_json(&key.private_pem)).expect("parse test account")
}

/// A client wired to `transport` with the standard test scope.
fn client(transport: Arc<MockTransport>) -> GoogleAuthClient {
    GoogleAuthClient::new(service_account(), vec![SCOPE.to_string()], None)
        .expect("build client")
        .with_transport(transport)
}

/// A `200 OK` token body.
fn token_body() -> String {
    format!(
        r#"{{"access_token":"ya29.mock-token","expires_in":3600,"token_type":"Bearer","scope":"{SCOPE}"}}"#
    )
}

/// The assertion from a recorded request, decoded and signature-verified.
fn recorded_claims(transport: &MockTransport) -> crate::claims::AssertionClaims {
    let (_, fields) = transport.last().expect("request recorded");
    let assertion = fields
        .iter()
        .find(|pair| pair.0 == "assertion")
        .map(|pair| pair.1.clone())
        .expect("assertion field present");
    decode_assertion(&assertion, &key_pair().public_pem)
}

/// Positive: the assertion is exchanged once and the token is cached.
#[tokio::test]
async fn exchanges_assertion_and_caches_token() {
    let transport = Arc::new(MockTransport::ok(&token_body()));
    let client = client(Arc::clone(&transport));

    let first = client.token().await.expect("first token");
    let second = client.token().await.expect("second token");

    assert_eq!(first.value(), "ya29.mock-token");
    assert_eq!(first.token_type(), "Bearer");
    assert!(Arc::ptr_eq(&first, &second), "cache must reuse the token");
    assert_eq!(transport.call_count(), 1, "one HTTP exchange expected");

    let (url, fields) = transport.last().expect("request recorded");
    assert_eq!(url, "https://oauth2.googleapis.com/token");
    let grant = fields
        .iter()
        .find(|pair| pair.0 == "grant_type")
        .expect("grant_type field");
    assert_eq!(grant.1, super::JWT_BEARER_GRANT);

    let claims = recorded_claims(&transport);
    assert_eq!(claims.iss, "test@test-project.iam.gserviceaccount.com");
    assert_eq!(claims.scope, SCOPE);
    assert_eq!(claims.aud, "https://oauth2.googleapis.com/token");
}

/// Two concurrent callers share exactly one token exchange (single-flight).
#[tokio::test]
async fn concurrent_calls_share_one_refresh() {
    let transport =
        Arc::new(MockTransport::ok(&token_body()).with_delay(Duration::from_millis(50)));
    let client = client(Arc::clone(&transport));

    let (first, second) = tokio::join!(client.token(), client.token());
    first.expect("first token");
    second.expect("second token");

    assert_eq!(transport.call_count(), 1, "single-flight refresh expected");
}

/// The cache renews once 80% of the token's lifetime has elapsed.
#[tokio::test]
async fn refreshes_after_eighty_percent_lifetime() {
    let transport = Arc::new(MockTransport::ok(&token_body()));
    let clock = Arc::new(ManualClock::new(Utc::now()));
    let client = GoogleAuthClient::new(service_account(), vec![SCOPE.to_string()], None)
        .expect("build client")
        .with_transport(Arc::clone(&transport) as Arc<dyn TokenTransport>)
        .with_clock(Arc::clone(&clock) as Arc<dyn Clock>);

    client.token().await.expect("first token");
    assert_eq!(transport.call_count(), 1);

    clock.advance(2879);
    client.token().await.expect("cached token");
    assert_eq!(transport.call_count(), 1, "cache must hold before 80%");

    clock.advance(1);
    client.token().await.expect("refreshed token");
    assert_eq!(transport.call_count(), 2, "refresh at 80% lifetime");
}

/// The impersonation subject is forwarded as the assertion `sub` claim.
#[tokio::test]
async fn subject_is_forwarded_in_assertion() {
    let transport = Arc::new(MockTransport::ok(&token_body()));
    let client = GoogleAuthClient::new(
        service_account(),
        vec![SCOPE.to_string()],
        Some("admin@example.com".to_string()),
    )
    .expect("build client")
    .with_transport(Arc::clone(&transport) as Arc<dyn TokenTransport>);

    client.token().await.expect("token");
    let claims = recorded_claims(&transport);
    assert_eq!(claims.sub.as_deref(), Some("admin@example.com"));
}

/// A non-success status is a typed error carrying the OAuth error code.
#[tokio::test]
async fn non_success_status_is_typed_error() {
    let body = r#"{"error":"invalid_grant","error_description":"Invalid JWT Signature."}"#;
    let client = client(Arc::new(MockTransport::returning(400, body)));
    let error = client.token().await.expect_err("must fail");

    match error {
        GoogleAuthError::TokenEndpoint { status, message } => {
            assert_eq!(status, 400);
            assert!(message.contains("invalid_grant"), "message: {message}");
            assert!(!message.contains("access_token"));
        }
        other => panic!("expected TokenEndpoint, got {other:?}"),
    }
}

/// A non-JSON error body is never echoed, so a token can never leak.
#[tokio::test]
async fn opaque_error_body_is_not_echoed() {
    let body = r#"{"unexpected":"ya29.leaky-token"}"#;
    let client = client(Arc::new(MockTransport::returning(500, body)));
    let error = client.token().await.expect_err("must fail");

    let rendered = error.to_string();
    assert!(
        !rendered.contains("ya29.leaky-token"),
        "body leaked: {rendered}"
    );
    assert!(rendered.contains("500"), "status missing: {rendered}");
}

/// A PEM that is not an RSA key is a typed error without key leakage.
#[tokio::test]
async fn invalid_private_key_is_typed_error() {
    let transport = Arc::new(MockTransport::ok(&token_body()));
    let account = ServiceAccount::new(
        "a@b.iam.gserviceaccount.com",
        "-----BEGIN PRIVATE KEY-----\nnot-a-key\n-----END PRIVATE KEY-----",
        "https://oauth2.googleapis.com/token",
    )
    .expect("build account");
    let client = GoogleAuthClient::new(account, vec![SCOPE.to_string()], None)
        .expect("build client")
        .with_transport(transport);

    let error = client.token().await.expect_err("signing must fail");
    assert!(matches!(error, GoogleAuthError::InvalidPrivateKey { .. }));
    assert!(!error.to_string().contains("not-a-key"));
}

/// A client with no usable scope refuses to build.
#[test]
fn empty_scopes_are_rejected() {
    let error = GoogleAuthClient::new(service_account(), vec!["  ".to_string()], None)
        .expect_err("no scopes must fail");
    assert!(matches!(error, GoogleAuthError::MissingScopes));
}

/// The client is usable behind a `dyn GoogleTokenSource` handle.
#[tokio::test]
async fn works_as_dyn_token_source() {
    let transport = Arc::new(MockTransport::ok(&token_body()));
    let source: Arc<dyn GoogleTokenSource> = Arc::new(client(transport));
    let token = source.token().await.expect("token");
    assert_eq!(token.value(), "ya29.mock-token");
}
