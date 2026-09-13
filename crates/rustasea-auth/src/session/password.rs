/// Session-backed password-confirmation state.
///
/// Extracted from `session` to keep each module under the 500-line limit.
/// Implements the `confirm-password` half of the session guard: a
/// `password_confirmed_at` timestamp stored under a dedicated session key
/// (mirroring Laravel's `auth.password_confirmed_at`), bounded by the
/// `AUTH_PASSWORD_TIMEOUT` / `[auth] password_timeout` window.
///
/// The timestamp is **not** part of the identity payload
/// ([`super::SessionUser`]); it lives under its own key so a login/refresh
/// round-trip never has to carry it and a confirmation can be added or read
/// independently of the identity record.
use std::sync::Arc;

use tower_sessions::session::Session;
use tower_sessions::SessionStore;

use crate::error::{AuthError, Result};

use super::{SessionGuard, SessionUser};

impl<S: SessionStore> SessionGuard<S> {
    /// Open the stored session for `token`, failing closed when it is absent.
    ///
    /// Shared by the password-confirmation helpers so each reads the identity
    /// key first: a confirmation must never be written onto (or read from) a
    /// session that does not already hold an authenticated principal.
    async fn open_session(&self, token: &str) -> Result<Session> {
        let id = Self::parse_id(token)?;
        let session = Session::new(Some(id), Arc::clone(&self.store), None);
        let user_key = self.policy.user_key();
        let user: Option<SessionUser> = session
            .get(&user_key)
            .await
            .map_err(|_| AuthError::StoreUnavailable)?;
        user.ok_or(AuthError::InvalidToken)?;
        Ok(session)
    }

    /// Read the stored password-confirmation timestamp for `session`.
    pub(super) async fn read_password_confirmed_at(
        &self,
        session: &Session,
    ) -> Result<Option<String>> {
        let key = self.policy.password_confirmed_key();
        session
            .get::<String>(&key)
            .await
            .map_err(|_| AuthError::StoreUnavailable)
    }

    /// Record a successful password confirmation on the session.
    ///
    /// Writes the current UNIX-seconds timestamp under
    /// [`super::SessionPolicy::password_confirmed_key`], mirroring Laravel's
    /// `auth.password_confirmed_at` session entry. The session must already
    /// hold an authenticated principal ([`super::SessionPolicy::user_key`]); a
    /// token that does not is rejected with [`AuthError::InvalidToken`] rather
    /// than silently minting a new session record.
    ///
    /// # Errors
    ///
    /// [`AuthError::InvalidToken`] for a malformed or non-authenticated token,
    /// or [`AuthError::StoreUnavailable`] when the store read/write fails.
    pub async fn confirm_password(&self, token: &str) -> Result<()> {
        self.confirm_password_at(token, crate::guard::principal::now_unix_secs())
            .await
    }

    /// Record a password confirmation with an explicit timestamp (test seam).
    ///
    /// Identical to [`SessionGuard::confirm_password`] but takes `now_unix_secs`
    /// so freshness can be asserted deterministically without a real clock.
    ///
    /// # Errors
    ///
    /// See [`SessionGuard::confirm_password`].
    pub async fn confirm_password_at(&self, token: &str, now_unix_secs: i64) -> Result<()> {
        let session = self.open_session(token).await?;
        let key = self.policy.password_confirmed_key();
        session
            .insert(&key, now_unix_secs.to_string())
            .await
            .map_err(|_| AuthError::StoreUnavailable)?;
        session
            .save()
            .await
            .map_err(|_| AuthError::StoreUnavailable)?;
        Ok(())
    }

    /// Read the session's stored password-confirmation timestamp, if any.
    ///
    /// # Errors
    ///
    /// [`AuthError::InvalidToken`] for a malformed or non-authenticated token,
    /// or [`AuthError::StoreUnavailable`] when the store read fails.
    pub async fn password_confirmed_at(&self, token: &str) -> Result<Option<String>> {
        let session = self.open_session(token).await?;
        self.read_password_confirmed_at(&session).await
    }

    /// Whether the session's password confirmation is within `timeout_secs` of
    /// `now_unix_secs`.
    ///
    /// Fail-closed: a session with no confirmation, an unparsable one, or a
    /// future-dated one returns `Ok(false)`.
    ///
    /// # Errors
    ///
    /// [`AuthError::InvalidToken`] for a malformed or non-authenticated token,
    /// or [`AuthError::StoreUnavailable`] when the store read fails.
    pub async fn is_password_confirmed_within(
        &self,
        token: &str,
        timeout_secs: u64,
        now_unix_secs: i64,
    ) -> Result<bool> {
        let confirmed = self.password_confirmed_at(token).await?;
        Ok(crate::guard::principal::timestamp_is_fresh(
            confirmed.as_deref(),
            timeout_secs,
            now_unix_secs,
        ))
    }
}
