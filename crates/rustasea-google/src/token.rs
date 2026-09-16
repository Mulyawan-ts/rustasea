//! Cached OAuth2 access token + refresh-threshold math.
//!
//! [`AccessToken`] records the token value, its type, the granted scopes, and
//! the issue/expiry instants. [`AccessToken::needs_refresh`] answers the
//! question the client actually asks: given "now" and a refresh ratio (default
//! 80% of the token's lifetime), should the cache be renewed?

use std::fmt;

use chrono::{DateTime, Duration, Utc};

/// A bearer access token obtained from the token endpoint.
///
/// The raw token value is private and masked in `Debug`. Use
/// [`AccessToken::value`] to read it for an outbound `Authorization` header.
#[derive(Clone)]
pub struct AccessToken {
    /// Raw bearer token value.
    value: String,
    /// Token type reported by the endpoint (`Bearer`).
    token_type: String,
    /// Scopes granted for this token.
    scopes: Vec<String>,
    /// Instant the token was issued.
    issued_at: DateTime<Utc>,
    /// Instant the token expires.
    expires_at: DateTime<Utc>,
}

impl AccessToken {
    /// Build a token from its exchange result.
    #[must_use]
    pub fn new(
        value: impl Into<String>,
        token_type: impl Into<String>,
        scopes: Vec<String>,
        issued_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Self {
        Self {
            value: value.into(),
            token_type: token_type.into(),
            scopes,
            issued_at,
            expires_at,
        }
    }

    /// The raw bearer token value.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// The token type reported by the endpoint (`Bearer`).
    #[must_use]
    pub fn token_type(&self) -> &str {
        &self.token_type
    }

    /// The scopes granted for this token.
    #[must_use]
    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }

    /// The instant the token was issued.
    #[must_use]
    pub fn issued_at(&self) -> DateTime<Utc> {
        self.issued_at
    }

    /// The instant the token expires.
    #[must_use]
    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }

    /// Whether the token is expired (or has not started) at `now`.
    #[must_use]
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        now >= self.expires_at
    }

    /// Whether the token should be refreshed at `now`.
    ///
    /// The token is refreshed once `ratio` of its lifetime has elapsed (0.8 by
    /// default), leaving a safety margin over the raw expiry. A non-positive or
    /// degenerate lifetime is treated as always-needing-refresh so a clock skew
    /// can never serve a stale token.
    #[must_use]
    pub fn needs_refresh(&self, now: DateTime<Utc>, ratio: f64) -> bool {
        if self.expires_at <= self.issued_at {
            return true;
        }
        let lifetime_ms = (self.expires_at - self.issued_at).num_milliseconds();
        let threshold_ms = (lifetime_ms as f64 * ratio).round() as i64;
        let refresh_at = self.issued_at + Duration::milliseconds(threshold_ms);
        now >= refresh_at
    }
}

impl fmt::Debug for AccessToken {
    /// Render the token with its value masked.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessToken")
            .field("value", &"[REDACTED]")
            .field("token_type", &self.token_type)
            .field("scopes", &self.scopes)
            .field("issued_at", &self.issued_at)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh token is neither expired nor due for refresh.
    #[test]
    fn fresh_token_needs_no_refresh() {
        let issued = Utc::now();
        let token = AccessToken::new(
            "ya29.secret",
            "Bearer",
            vec!["scope".to_string()],
            issued,
            issued + Duration::seconds(100),
        );
        assert!(!token.is_expired(issued));
        assert!(!token.needs_refresh(issued, 0.8));
        assert!(!token.needs_refresh(issued + Duration::seconds(79), 0.8));
    }

    /// The token is due for refresh once 80% of its lifetime has elapsed.
    #[test]
    fn needs_refresh_at_eighty_percent_lifetime() {
        let issued = Utc::now();
        let token = AccessToken::new(
            "ya29.secret",
            "Bearer",
            vec![],
            issued,
            issued + Duration::seconds(100),
        );
        assert!(token.needs_refresh(issued + Duration::seconds(80), 0.8));
        assert!(token.is_expired(issued + Duration::seconds(100)));
    }

    /// A degenerate lifetime is always refreshable.
    #[test]
    fn zero_lifetime_always_refreshes() {
        let issued = Utc::now();
        let token = AccessToken::new("v", "Bearer", vec![], issued, issued);
        assert!(token.needs_refresh(issued, 0.8));
    }

    /// `Debug` masks the token value.
    #[test]
    fn debug_redacts_value() {
        let issued = Utc::now();
        let token = AccessToken::new(
            "ya29.super-secret",
            "Bearer",
            vec![],
            issued,
            issued + Duration::seconds(10),
        );
        let rendered = format!("{token:?}");
        assert!(
            !rendered.contains("super-secret"),
            "value leaked: {rendered}"
        );
        assert!(rendered.contains("[REDACTED]"));
    }
}
