//! `NewDeviceNotification` — the queued "new device sign-in" mailable.
//!
//! Mirrors rappasoft/laravel-authentication-log's `NewDeviceLogin` notification:
//! when a successful login arrives from an IP the account has never used, the
//! account owner is emailed. Delivery is **queued** (never inline), so a slow or
//! failing mail transport cannot delay or fail the HTTP login response.
//!
//! The type implements [`rustasea_mail::MailNotification`] (a serializable
//! [`rustasea_mail::Mailable`]), which is what [`rustasea_mail::Mail::queue`]
//! requires; [`crate::QueuedMailNotifier`] wraps the construction + enqueue.

use rustasea_mail::{MailAddress, MailNotification, Mailable};
use serde::{Deserialize, Serialize};

/// The new-device sign-in notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewDeviceNotification {
    /// Authenticated user id (informational; used in the body).
    pub user_id: String,
    /// Recipient email address.
    pub email: String,
    /// Client IP address the login arrived from.
    pub ip_address: String,
    /// Client `User-Agent`, when present.
    pub user_agent: Option<String>,
}

impl NewDeviceNotification {
    /// Build a notification for `user_id` at `email` from `ip_address`.
    pub fn new(
        user_id: impl Into<String>,
        email: impl Into<String>,
        ip_address: impl Into<String>,
        user_agent: Option<String>,
    ) -> Self {
        Self {
            user_id: user_id.into(),
            email: email.into(),
            ip_address: ip_address.into(),
            user_agent,
        }
    }
}

impl Mailable for NewDeviceNotification {
    /// Subject line for the notification.
    fn subject(&self) -> String {
        "New sign-in to your account".to_string()
    }

    /// Recipients — the account owner.
    fn to(&self) -> Vec<MailAddress> {
        vec![MailAddress::from_email(self.email.clone())]
    }

    /// HTML body describing the new device sign-in.
    fn html_body(&self) -> String {
        let agent = self.user_agent.as_deref().unwrap_or("an unknown device");
        format!(
            "<p>A new sign-in to your account was detected.</p>\
             <ul>\
             <li>IP address: {ip}</li>\
             <li>Device: {agent}</li>\
             </ul>\
             <p>If this was you, no action is needed. If not, reset your password immediately.</p>",
            ip = self.ip_address,
            agent = agent,
        )
    }

    /// Plain-text body for mail clients that do not render HTML.
    fn text_body(&self) -> Option<String> {
        let agent = self.user_agent.as_deref().unwrap_or("an unknown device");
        Some(format!(
            "A new sign-in to your account was detected.\nIP address: {ip}\nDevice: {agent}\n\
             If this was not you, reset your password immediately.",
            ip = self.ip_address,
            agent = agent,
        ))
    }
}

impl MailNotification for NewDeviceNotification {
    /// The targeted user id, so a deleted-account queue job can be suppressed.
    fn model_id(&self) -> Option<String> {
        Some(self.user_id.clone())
    }

    /// Diagnostic name surfaced when a queued delivery is suppressed.
    fn notification_name(&self) -> &'static str {
        "NewDeviceNotification"
    }
}
