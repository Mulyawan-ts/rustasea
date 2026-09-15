//! Authentication-log emission (ADOPT-003).
//!
//! Split out of [`super`] so `auth.rs` stays under the 500-line cap. Each
//! helper builds an [`AuthLogEvent`] and hands it to the process-wide logger
//! through the best-effort [`rustasea_authlog::record_event`] seam:
//!
//! * recording **no-ops** when no logger is installed (CLI/queue/test);
//! * a storage failure is swallowed, because an audit-write hiccup must never
//!   break an authentication that already succeeded.
//!
//! The client IP is the same `peer_ip` value the throttle uses (honouring
//! `X-Forwarded-For` only behind trusted proxies); the user agent is the raw
//! `User-Agent` header, read here so the call sites stay one-liners.
//!
//! # Lockout rows
//!
//! Every blocked attempt records a `lockout` row (not just the first), so the
//! history reflects the true volume of throttled attempts. This is a deliberate
//! choice — the alternative (one row per lockout window) would hide repeat
//! abuse.

use axum::http::{header, HeaderMap};

use rustasea::auth::{Guard, SessionGuard};
use rustasea_authlog::{AuthLogEvent, AuthLogEventKind};

/// The `User-Agent` request header as a lossy string, when present.
pub(crate) fn user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .filter(|value| !value.is_empty())
}

/// Best-effort emit: a storage failure never breaks the auth flow.
async fn emit(event: &AuthLogEvent) {
    let _ = rustasea_authlog::record_event(event).await;
}

/// Record a successful login, resolving the user id from the issued session.
///
/// The user id is read back through [`Guard::parse`] on the freshly issued
/// token; when that fails (an anomalous store state) the row is still written
/// with the submitted email and no user id rather than dropped.
pub(crate) async fn login_succeeded(
    guard: &SessionGuard,
    access_token: &str,
    submitted_email: &str,
    ip_address: &str,
    headers: &HeaderMap,
) {
    let guard_name = guard.name().to_string();
    let (user_id, email) = match guard.parse(access_token).await {
        Ok(user) => (
            Some(user.id),
            user.email.unwrap_or_else(|| submitted_email.to_string()),
        ),
        Err(_) => (None, submitted_email.to_string()),
    };
    let event = AuthLogEvent {
        kind: AuthLogEventKind::LoginSucceeded,
        user_id,
        email: Some(email),
        guard_name: Some(guard_name),
        ip_address: Some(ip_address.to_string()),
        user_agent: user_agent(headers),
    };
    emit(&event).await;
}

/// Record a failed login (unknown email or wrong password).
pub(crate) async fn login_failed(
    email: &str,
    guard_name: &str,
    ip_address: &str,
    headers: &HeaderMap,
) {
    emit(&AuthLogEvent::login_failed(
        Some(email.to_string()),
        Some(guard_name.to_string()),
        Some(ip_address.to_string()),
        user_agent(headers),
    ))
    .await;
}

/// Record a throttled (locked-out) login attempt.
pub(crate) async fn lockout(email: &str, guard_name: &str, ip_address: &str, headers: &HeaderMap) {
    emit(&AuthLogEvent::lockout(
        Some(email.to_string()),
        Some(guard_name.to_string()),
        Some(ip_address.to_string()),
        user_agent(headers),
    ))
    .await;
}

/// Record a logout, resolving the user id from the still-live session.
///
/// Must be called **before** [`Guard::logout`] destroys the session, otherwise
/// the user id can no longer be resolved and the open login row would never be
/// closed.
pub(crate) async fn logout(
    guard: Option<&SessionGuard>,
    session_id: Option<&str>,
    ip_address: &str,
    headers: &HeaderMap,
) {
    let (user_id, guard_name) = match (guard, session_id) {
        (Some(guard), Some(session_id)) => {
            let name = guard.name().to_string();
            match guard.parse(session_id).await {
                Ok(user) => (Some(user.id), Some(name)),
                Err(_) => (None, Some(name)),
            }
        }
        _ => (None, None),
    };
    emit(&AuthLogEvent::logout(
        user_id,
        guard_name,
        Some(ip_address.to_string()),
        user_agent(headers),
    ))
    .await;
}
