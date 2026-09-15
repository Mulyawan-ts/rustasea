/// Authenticated principal returned by `Guard::parse`.
///
/// Beyond the identity triple (`id`, `email`, `guard`) the principal carries the
/// two pieces of state the HTTP authorization gates need:
///
/// * `email_verified_at` — the `users.email_verified_at` timestamp, or `None`
///   for an unverified account. `require_verified` fails closed on `None`.
/// * `password_confirmed_at` — the session's `auth.password_confirmed_at`
///   timestamp, or `None` when the password was never re-confirmed this
///   session. `require_password_confirmed` fails closed on `None` or a stale
///   value.
///
/// Both fields are optional so every existing construction site
/// ([`AuthUser::new`]) keeps compiling; populate them with
/// [`AuthUser::with_email_verified_at`] /
/// [`AuthUser::with_password_confirmed_at`].
///
/// * `timezone` — the user's preferred IANA timezone (`users.timezone`), or
///   `None` when the account has no explicit preference. Populate it with
///   [`AuthUser::with_timezone`]; the timezone mapper falls through to the
///   session/header/app default when it is `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthUser {
    /// Primary key (UUID string) of the authenticated record.
    pub id: String,
    /// Login identifier (email for JWT/session guards).
    pub email: Option<String>,
    /// Guard name that produced this principal.
    pub guard: String,
    /// `users.email_verified_at` timestamp, or `None` when unverified.
    pub email_verified_at: Option<String>,
    /// Session `auth.password_confirmed_at` timestamp, or `None`.
    pub password_confirmed_at: Option<String>,
    /// Preferred IANA timezone (`users.timezone`), or `None` for no preference.
    pub timezone: Option<String>,
}

impl AuthUser {
    /// Build a principal for a custom guard's `parse`/`user` results.
    ///
    /// The verification and confirmation fields default to `None`, so a
    /// principal built here is unverified and unconfirmed until a caller
    /// populates them with the `with_*` builders.
    ///
    /// # Custom guard example
    ///
    /// ```rust
    /// use rustasea_auth::{AuthUser, Guard, AuthError};
    ///
    /// /// Minimal API-key guard: `"secret"` is the only valid key.
    /// pub struct ApiKeyGuard;
    ///
    /// impl Guard for ApiKeyGuard {
    ///     fn name(&self) -> &str { "api" }
    ///     fn login<'a>(&'a self, _: &'a rustasea_auth::Credentials)
    ///         -> std::pin::Pin<Box<dyn std::future::Future<Output = rustasea_auth::error::Result<rustasea_auth::Token>> + Send + 'a>>
    ///     { Box::pin(async move { Err(AuthError::BadCredentials) }) }
    ///     fn login_using_id<'a>(&'a self, _: &'a str)
    ///         -> std::pin::Pin<Box<dyn std::future::Future<Output = rustasea_auth::error::Result<rustasea_auth::Token>> + Send + 'a>>
    ///     { Box::pin(async move { Err(AuthError::BadCredentials) }) }
    ///     fn parse<'a>(&'a self, token: &'a str)
    ///         -> std::pin::Pin<Box<dyn std::future::Future<Output = rustasea_auth::error::Result<AuthUser>> + Send + 'a>>
    ///     {
    ///         Box::pin(async move {
    ///             if token == "secret" {
    ///                 Ok(AuthUser::new("user-1", Some("api@example.com"), "api"))
    ///             } else {
    ///                 Err(AuthError::InvalidToken)
    ///             }
    ///         })
    ///     }
    ///     fn refresh<'a>(&'a self, _: &'a str)
    ///         -> std::pin::Pin<Box<dyn std::future::Future<Output = rustasea_auth::error::Result<rustasea_auth::Token>> + Send + 'a>>
    ///     { Box::pin(async move { Err(AuthError::BadCredentials) }) }
    ///     fn logout<'a>(&'a self, _: &'a str)
    ///         -> std::pin::Pin<Box<dyn std::future::Future<Output = rustasea_auth::error::Result<()>> + Send + 'a>>
    ///     { Box::pin(async move { Ok(()) }) }
    ///     fn user<'a>(&'a self)
    ///         -> std::pin::Pin<Box<dyn std::future::Future<Output = rustasea_auth::error::Result<Option<AuthUser>>> + Send + 'a>>
    ///     { Box::pin(async move { Ok(None) }) }
    ///     fn id<'a>(&'a self)
    ///         -> std::pin::Pin<Box<dyn std::future::Future<Output = rustasea_auth::error::Result<Option<String>>> + Send + 'a>>
    ///     { Box::pin(async move { Ok(None) }) }
    /// }
    /// ```
    pub fn new(
        id: impl Into<String>,
        email: Option<impl Into<String>>,
        guard: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            email: email.map(Into::into),
            guard: guard.into(),
            email_verified_at: None,
            password_confirmed_at: None,
            timezone: None,
        }
    }

    /// Populate the `email_verified_at` timestamp (builder form).
    pub fn with_email_verified_at(mut self, at: Option<impl Into<String>>) -> Self {
        self.email_verified_at = at.map(Into::into);
        self
    }

    /// Populate the session `password_confirmed_at` timestamp (builder form).
    pub fn with_password_confirmed_at(mut self, at: Option<impl Into<String>>) -> Self {
        self.password_confirmed_at = at.map(Into::into);
        self
    }

    /// Populate the preferred IANA timezone (`users.timezone`), builder form.
    pub fn with_timezone(mut self, timezone: Option<impl Into<String>>) -> Self {
        self.timezone = timezone.map(Into::into);
        self
    }

    /// Whether the email is verified (`email_verified_at` is present).
    ///
    /// Fail-closed: a missing timestamp means unverified.
    pub fn is_email_verified(&self) -> bool {
        self.email_verified_at.is_some()
    }

    /// Whether the password was confirmed within `timeout_secs` of
    /// `now_unix_secs`.
    ///
    /// Fail-closed: a missing, unparsable, or future-dated timestamp returns
    /// `false`, so a clock skew or a poisoned value can never widen the
    /// confirmation window. The stored value may be either a bare
    /// integer-seconds string (the format the session helper writes) or an
    /// RFC 3339 datetime (the `users.email_verified_at` column format).
    pub fn is_password_confirmed_within(&self, timeout_secs: u64, now_unix_secs: i64) -> bool {
        timestamp_is_fresh(
            self.password_confirmed_at.as_deref(),
            timeout_secs,
            now_unix_secs,
        )
    }

    /// Whether the password was confirmed within `timeout_secs` of the current
    /// wall clock.
    ///
    /// Clock-backed convenience over [`AuthUser::is_password_confirmed_within`],
    /// so HTTP middleware can gate a request without depending on a date/time
    /// crate of its own. Prefer the explicit-`now` form in tests.
    pub fn is_password_confirmed(&self, timeout_secs: u64) -> bool {
        self.is_password_confirmed_within(timeout_secs, now_unix_secs())
    }
}

/// Current UNIX time in whole seconds.
pub(crate) fn now_unix_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Whether a stored confirmation `timestamp` is within `timeout_secs` of
/// `now_unix_secs`.
///
/// Shared by [`AuthUser::is_password_confirmed_within`] and the session-backed
/// [`crate::session::SessionGuard::is_password_confirmed_within`] so both apply
/// the identical fail-closed rule (missing, unparsable, or future-dated values
/// are rejected).
pub(crate) fn timestamp_is_fresh(
    timestamp: Option<&str>,
    timeout_secs: u64,
    now_unix_secs: i64,
) -> bool {
    let Some(confirmed) = timestamp else {
        return false;
    };
    let Some(confirmed_unix) = parse_unix_seconds(confirmed) else {
        return false;
    };
    // A timestamp in the future cannot prove a fresh confirmation; treat it
    // as invalid rather than trusting it.
    if confirmed_unix > now_unix_secs {
        return false;
    }
    let age = (now_unix_secs - confirmed_unix) as u64;
    age <= timeout_secs
}

/// Parse a stored timestamp into UNIX seconds.
///
/// Accepts a bare integer-seconds string (written by the session
/// password-confirmation helper) or an RFC 3339 datetime (the database
/// timestamp format). Returns `None` for anything else, so callers fail closed.
fn parse_unix_seconds(value: &str) -> Option<i64> {
    let trimmed = value.trim();
    if let Ok(secs) = trimmed.parse::<i64>() {
        return Some(secs);
    }
    chrono::DateTime::parse_from_rfc3339(trimmed)
        .ok()
        .map(|dt| dt.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `new` leaves both optional fields empty and the predicates fail closed.
    #[test]
    fn new_principal_is_unverified_and_unconfirmed() {
        let user = AuthUser::new("user-1", Some("ada@example.com"), "session");
        assert_eq!(user.email_verified_at, None);
        assert_eq!(user.password_confirmed_at, None);
        assert!(!user.is_email_verified());
        assert!(!user.is_password_confirmed_within(10_800, 1_000_000));
    }

    /// The builders populate the fields and flip the predicates.
    #[test]
    fn builders_populate_state() {
        let user = AuthUser::new("user-1", Some("ada@example.com"), "session")
            .with_email_verified_at(Some("2026-01-01T00:00:00Z"))
            .with_password_confirmed_at(Some("1000"));
        assert!(user.is_email_verified());
        assert!(user.is_password_confirmed_within(10_800, 1_000 + 10_800));
    }

    /// A stale confirmation falls outside the window.
    #[test]
    fn stale_confirmation_is_rejected() {
        let user = AuthUser::new("user-1", None::<String>, "session")
            .with_password_confirmed_at(Some("1000"));
        // 10_801 seconds after the confirmation exceeds the 10_800 window.
        assert!(!user.is_password_confirmed_within(10_800, 1_000 + 10_801));
        // Exactly at the boundary is still fresh.
        assert!(user.is_password_confirmed_within(10_800, 1_000 + 10_800));
    }

    /// A future or unparsable timestamp fails closed.
    #[test]
    fn future_and_unparsable_timestamps_fail_closed() {
        let future = AuthUser::new("user-1", None::<String>, "session")
            .with_password_confirmed_at(Some("2000"));
        assert!(!future.is_password_confirmed_within(10_800, 1_000));

        let garbage = AuthUser::new("user-1", None::<String>, "session")
            .with_password_confirmed_at(Some("not-a-timestamp"));
        assert!(!garbage.is_password_confirmed_within(10_800, 2_000));
    }

    /// An RFC 3339 confirmation timestamp is accepted.
    #[test]
    fn rfc3339_confirmation_is_accepted() {
        let user = AuthUser::new("user-1", None::<String>, "session")
            .with_password_confirmed_at(Some("2026-01-01T00:00:00Z"));
        let confirmed = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("valid rfc3339")
            .timestamp();
        assert!(user.is_password_confirmed_within(60, confirmed + 30));
        assert!(!user.is_password_confirmed_within(60, confirmed + 61));
    }
}
