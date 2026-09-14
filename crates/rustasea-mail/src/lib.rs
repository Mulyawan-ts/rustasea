//! RustaSea Mail — Mailable/Mailer contracts, transports, and queued
//! notifications.
//!
//! Sprint 07 (M6) scope per sprint-07.md S07-T05: the [`Mailable`] contract,
//! pluggable [`Mailer`] transports (SMTP/Log/Array), the [`MailMessage`] model,
//! and [`MailNotification`] delivery through `rustasea-queue` with
//! `#[deleteWhenMissingModels]` suppression (FR-605, US-M6-04).
//!
//! Laravel 13.x `mail.php` parity lives in [`config`]: [`MailConfig`] parses the
//! `[mail]` table from `config/mail.toml` and [`mailer_from_config`] builds a
//! ready [`Mailer`] from it (honouring `MAIL_*` environment overrides).
//!
//! # Known deviation: process-wide `set_mailer` (ADR-0007)
//!
//! This crate predates the mail template hook and exposes a process-wide mailer
//! registry through [`Mail::set_mailer`] / [`mailer`]. That conflicts with
//! ADR-0007 ("Facades Replaced by AppState Arc"), which forbids global facades
//! in favour of `AppState`-injected dependencies: a global mailer prevents
//! per-test isolation and hides the mailer's lifecycle from the container.
//!
//! The migration path is the **explicit injection** constructors added here —
//! [`Mail::send_with`] and [`TemplateMailable`] — which take an
//! `Arc<dyn Mailer>` / `Arc<dyn ViewEngine>` directly, so a caller can thread
//! the mailer through `AppState` instead of reaching for the global. The
//! global remains for backward compatibility and for queued notifications
//! ([`QueuedNotification`] resolves its mailer at worker time, where no
//! `AppState` is in scope); new code should prefer the explicit path.
//!
//! ```no_run
//! use std::sync::Arc;
//! use rustasea_mail::{ArrayMailer, Mail, MailAddress, Mailable};
//!
//! struct Welcome { name: String }
//! impl Mailable for Welcome {
//!     fn subject(&self) -> String { "Welcome".into() }
//!     fn to(&self) -> Vec<MailAddress> { vec![MailAddress::from("ada@example.com")] }
//!     fn html_body(&self) -> String { format!("<h1>Hi {}</h1>", self.name) }
//! }
//!
//! # async fn run() -> rustasea_mail::Result<()> {
//! Mail::set_mailer(Arc::new(ArrayMailer::new()));
//! Mail::send(&Welcome { name: "Ada".into() }).await?;
//! # Ok(())
//! # }
//! ```

pub mod address;
pub mod config;
pub mod error;
pub mod mailable;
pub mod mailer;
pub mod message;
pub mod notification;
#[cfg(feature = "smtp")]
pub mod smtp;
#[cfg(feature = "templates")]
pub mod template;

use std::sync::Arc;

pub use address::MailAddress;
pub use config::{mailer_from_config, FromConfig, MailConfig, MailerConfig, DEFAULT_MAILER};
pub use error::{MailConfigError, MailError, Result};
pub use mailable::Mailable;
pub use mailer::{mailer, set_mailer, ArrayMailer, FailoverMailer, FromMailer, LogMailer, Mailer};
pub use message::MailMessage;
pub use notification::{MailNotification, QueuedNotification};
#[cfg(feature = "smtp")]
pub use smtp::SmtpMailer;
#[cfg(feature = "templates")]
pub use template::TemplateMailable;
#[cfg(feature = "templates")]
pub use template::{MinijinjaEngine, ViewEngine, ViewError};

/// Entry point for sending and queueing mail.
pub struct Mail;

impl Mail {
    /// Install the process-wide mailer used by [`Mail::send`] and queued
    /// notifications.
    pub fn set_mailer(mailer: Arc<dyn Mailer>) {
        mailer::set_mailer(mailer);
    }

    /// The installed process-wide mailer, when one has been set.
    pub fn mailer() -> Option<Arc<dyn Mailer>> {
        mailer::mailer()
    }

    /// Deliver a mailable immediately through the process-wide mailer.
    pub async fn send<M: Mailable>(mailable: &M) -> Result<()> {
        let mailer =
            mailer::mailer().ok_or_else(|| MailError::Transport("no mailer installed".into()))?;
        mailer.send(mailable.build()).await
    }

    /// Deliver a mailable immediately through an explicitly supplied mailer.
    ///
    /// This is the ADR-0007-compliant path: a caller threads the mailer (for
    /// example from `AppState`) instead of relying on the process-wide registry.
    /// See the crate docs' "Known deviation" note for the migration rationale.
    pub async fn send_with<M: Mailable>(mailer: &Arc<dyn Mailer>, mailable: &M) -> Result<()> {
        mailer.send(mailable.build()).await
    }

    /// Deliver a pre-built [`MailMessage`] through an explicitly supplied mailer.
    ///
    /// Pairs with [`TemplateMailable::try_build`], whose fallible rendering must
    /// complete before delivery; the caller passes the resolved mailer directly.
    pub async fn deliver_with(mailer: &Arc<dyn Mailer>, message: MailMessage) -> Result<()> {
        mailer.send(message).await
    }

    /// Queue a notification for asynchronous delivery.
    ///
    /// The returned handle resolves through the `notifications` route; call
    /// [`rustasea_queue::Queue::route_sync`] (or `route`) at boot so dispatch
    /// can resolve the job type.
    pub async fn queue<M: MailNotification>(mailable: M) -> Result<rustasea_queue::JobId> {
        Ok(rustasea_queue::Queue::dispatch(QueuedNotification::new(mailable)).await?)
    }
}
