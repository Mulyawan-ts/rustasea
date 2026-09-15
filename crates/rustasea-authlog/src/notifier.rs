//! New-device notification seam and the default queued-mail implementation.
//!
//! The recorder checks for a prior login from the same IP after a successful
//! login; on the first sighting it calls [`NewDeviceNotifier::notify`]. The
//! trait is the seam (tests inject a recording fake), and
//! [`QueuedMailNotifier`] is the shipped implementation: it builds a
//! [`NewDeviceNotification`] and **queues** it through
//! [`rustasea_mail::Mail::queue`].

use async_trait::async_trait;

use crate::error::{AuthLogError, Result};
use crate::notification::NewDeviceNotification;

/// Callback invoked when a login arrives from a previously-unseen IP.
#[async_trait]
pub trait NewDeviceNotifier: Send + Sync + 'static {
    /// Notify the account owner of a sign-in from a new device.
    ///
    /// `email` is the recipient; `None` means the address is unknown and the
    /// implementation should no-op. Implementations must not panic.
    async fn notify(
        &self,
        user_id: &str,
        email: Option<&str>,
        ip_address: &str,
        user_agent: Option<&str>,
    ) -> Result<()>;
}

/// Default notifier — queues the new-device notification through `rustasea-mail`.
///
/// Delivery is **queued**, never inline: the login response must not block on a
/// mail transport. A mail error is mapped to [`AuthLogError::Mail`] and, per the
/// recorder's documented policy, never fails the underlying login.
#[derive(Debug, Default, Clone, Copy)]
pub struct QueuedMailNotifier;

#[async_trait]
impl NewDeviceNotifier for QueuedMailNotifier {
    /// Build and enqueue the new-device notification.
    async fn notify(
        &self,
        user_id: &str,
        email: Option<&str>,
        ip_address: &str,
        user_agent: Option<&str>,
    ) -> Result<()> {
        // Without a recipient there is nowhere to send the notice.
        let Some(email) = email else {
            return Ok(());
        };
        let notification =
            NewDeviceNotification::new(user_id, email, ip_address, user_agent.map(str::to_string));
        rustasea_mail::Mail::queue(notification)
            .await
            .map(|_| ())
            .map_err(|error| AuthLogError::Mail(error.to_string()))
    }
}
