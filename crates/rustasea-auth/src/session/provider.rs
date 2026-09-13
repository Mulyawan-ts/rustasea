/// Async database-backed login for [`SessionGuard`].
///
/// Extracted from `session` to keep each module under the 500-line limit and to
/// keep the sync [`crate::users::UserLookup`] login path untouched. The sync
/// `Guard::login` remains the JWT/session test path; this module adds the
/// async, write-capable-provider path the HTTP login handler needs.
use std::future::Future;
use std::pin::Pin;

use crate::error::{AuthError, Result};
use crate::guard::{Credentials, Token};
use crate::users::UserProvider;

use super::{SessionGuard, SessionUser};
use tower_sessions::SessionStore;

impl<S: SessionStore> SessionGuard<S> {
    /// Verify credentials against an async [`UserProvider`] and issue a fresh
    /// session token.
    ///
    /// Unlike [`crate::guard::Guard::login`] (which reads a synchronous
    /// [`crate::users::UserLookup`]), this entry point awaits
    /// [`UserProvider::find_by_email`], so a database-backed provider can
    /// authenticate a login without a pre-loaded in-memory lookup.
    ///
    /// The flow is:
    ///
    /// 1. `provider.find_by_email(email)` → `None` is [`AuthError::BadCredentials`].
    /// 2. Verify the plaintext password against the stored PHC hash with the
    ///    guard's [`crate::verify::PasswordVerifier`] → a mismatch is
    ///    [`AuthError::BadCredentials`].
    /// 3. Persist a **fresh** session (the existing `persist()` always mints a
    ///    new [`tower_sessions::session::Id`] — the session-fixation defense).
    /// 4. Return the same [`Token`] shape as [`crate::guard::Guard::login`]
    ///    (`token_type == "Session"`).
    ///
    /// # Account-enumeration resistance
    ///
    /// An unknown email and a wrong password both return
    /// [`AuthError::BadCredentials`], byte-for-byte identical, so a caller
    /// cannot distinguish "no such user" from "bad password".
    ///
    /// # Errors
    ///
    /// [`AuthError::BadCredentials`] for unknown email or wrong password, and
    /// [`AuthError::StoreUnavailable`] when the session store write fails.
    pub fn login_with_provider<'a>(
        &'a self,
        provider: &'a dyn UserProvider,
        credentials: &'a Credentials,
    ) -> Pin<Box<dyn Future<Output = Result<Token>> + Send + 'a>> {
        Box::pin(async move {
            let record = provider
                .find_by_email(&credentials.email)
                .await?
                .ok_or(AuthError::BadCredentials)?;
            // Constant-time argon2 verify; the same error is returned for an
            // unknown email and a wrong password so account enumeration via
            // timing or error shape is not possible.
            if !self
                .verifier
                .verify(&record.password_hash, &credentials.password)
            {
                return Err(AuthError::BadCredentials);
            }
            let user = SessionUser {
                id: record.id,
                email: Some(record.email),
                email_verified_at: record.email_verified_at,
            };
            let id = self.persist(&user).await?;
            Ok(self.issue(&id))
        })
    }
}
