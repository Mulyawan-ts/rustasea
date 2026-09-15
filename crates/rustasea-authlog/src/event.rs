//! Authentication event types — the typed input to the recorder.
//!
//! A single [`AuthLogEvent`] models one authentication occurrence (a login
//! attempt, a lockout, or a logout). The HTTP layer builds one per request and
//! hands it to [`crate::AuthenticationLogLogger::record`]; the recorder maps it
//! onto the `authentication_log` table.

/// The kind of authentication event recorded.
///
/// The `as_str` strings are the stable values persisted in the
/// `authentication_log.event` column and must not change once released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthLogEventKind {
    /// A credential check succeeded.
    LoginSucceeded,
    /// A credential check failed (unknown email or wrong password).
    LoginFailed,
    /// A login attempt was blocked by the rate limiter.
    Lockout,
    /// An authenticated session was torn down.
    Logout,
}

impl AuthLogEventKind {
    /// Stable persisted string for this kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthLogEventKind::LoginSucceeded => "login_succeeded",
            AuthLogEventKind::LoginFailed => "login_failed",
            AuthLogEventKind::Lockout => "lockout",
            AuthLogEventKind::Logout => "logout",
        }
    }

    /// Parse a persisted `event` string back into a kind.
    ///
    /// Returns `None` for an unrecognised value so a forward-compatible reader
    /// never panics on a row written by a newer version.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "login_succeeded" => Some(AuthLogEventKind::LoginSucceeded),
            "login_failed" => Some(AuthLogEventKind::LoginFailed),
            "lockout" => Some(AuthLogEventKind::Lockout),
            "logout" => Some(AuthLogEventKind::Logout),
            _ => None,
        }
    }

    /// Whether this kind represents a successful authentication.
    pub fn is_successful(&self) -> bool {
        matches!(
            self,
            AuthLogEventKind::LoginSucceeded | AuthLogEventKind::Logout
        )
    }
}

impl std::fmt::Display for AuthLogEventKind {
    /// Render the stable persisted string.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One authentication occurrence handed to the recorder.
#[derive(Debug, Clone)]
pub struct AuthLogEvent {
    /// The event kind.
    pub kind: AuthLogEventKind,
    /// Authenticated user id, when known (a failed attempt has none).
    pub user_id: Option<String>,
    /// Login identifier (email), when known.
    pub email: Option<String>,
    /// Guard name that produced the event (`session`, `jwt`, custom).
    pub guard_name: Option<String>,
    /// Client IP address, when resolvable.
    pub ip_address: Option<String>,
    /// Client `User-Agent` header, when present.
    pub user_agent: Option<String>,
}

impl AuthLogEvent {
    /// Build a successful-login event.
    pub fn login_succeeded(
        user_id: impl Into<String>,
        email: Option<String>,
        guard_name: Option<String>,
        ip_address: Option<String>,
        user_agent: Option<String>,
    ) -> Self {
        Self {
            kind: AuthLogEventKind::LoginSucceeded,
            user_id: Some(user_id.into()),
            email,
            guard_name,
            ip_address,
            user_agent,
        }
    }

    /// Build a failed-login event (no user id — the account may not exist).
    pub fn login_failed(
        email: Option<String>,
        guard_name: Option<String>,
        ip_address: Option<String>,
        user_agent: Option<String>,
    ) -> Self {
        Self {
            kind: AuthLogEventKind::LoginFailed,
            user_id: None,
            email,
            guard_name,
            ip_address,
            user_agent,
        }
    }

    /// Build a lockout event (the attempt was throttled before credential work).
    pub fn lockout(
        email: Option<String>,
        guard_name: Option<String>,
        ip_address: Option<String>,
        user_agent: Option<String>,
    ) -> Self {
        Self {
            kind: AuthLogEventKind::Lockout,
            user_id: None,
            email,
            guard_name,
            ip_address,
            user_agent,
        }
    }

    /// Build a logout event.
    pub fn logout(
        user_id: Option<String>,
        guard_name: Option<String>,
        ip_address: Option<String>,
        user_agent: Option<String>,
    ) -> Self {
        Self {
            kind: AuthLogEventKind::Logout,
            user_id,
            email: None,
            guard_name,
            ip_address,
            user_agent,
        }
    }
}
