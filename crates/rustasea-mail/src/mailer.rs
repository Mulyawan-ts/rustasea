//! Mailer transports and the process-wide mailer registry.

use std::any::Any;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use async_trait::async_trait;

use crate::address::MailAddress;
use crate::error::{MailError, Result};
use crate::message::MailMessage;

/// A transport that delivers fully-built messages.
///
/// [`Any`] is a supertrait so a boxed mailer can be downcast back to its
/// concrete type (see [`dyn Mailer::downcast_ref`](dyn Mailer)), which the
/// configuration factory and tests rely on.
#[async_trait]
pub trait Mailer: Send + Sync + Any + 'static {
    /// Deliver `message`, or return a transport error.
    async fn send(&self, message: MailMessage) -> Result<()>;
}

impl dyn Mailer {
    /// Downcast this mailer to a concrete type, when it is `T`.
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        let any: &dyn Any = self;
        any.downcast_ref::<T>()
    }
}

/// Mailer decorator that stamps a default `from` on messages that lack one.
///
/// Laravel applies `mail.from` globally; RustaSea keeps the [`Mailer`] contract
/// sender-agnostic, so the configuration factory wraps the transport in a
/// `FromMailer` when `[mail.from]` is set. A message that already carries its
/// own `from` is passed through unchanged.
pub struct FromMailer {
    /// Wrapped transport that performs the delivery.
    inner: Arc<dyn Mailer>,
    /// Sender applied to messages without their own `from`.
    from: MailAddress,
}

impl FromMailer {
    /// Wrap `inner`, stamping `from` on messages that lack a sender.
    pub fn new(inner: Arc<dyn Mailer>, from: impl Into<MailAddress>) -> Self {
        Self {
            inner,
            from: from.into(),
        }
    }

    /// The wrapped transport.
    pub fn inner(&self) -> &Arc<dyn Mailer> {
        &self.inner
    }

    /// The default sender applied to messages.
    pub fn from(&self) -> &MailAddress {
        &self.from
    }
}

#[async_trait]
impl Mailer for FromMailer {
    /// Apply the default `from` (when absent) and delegate to the transport.
    async fn send(&self, mut message: MailMessage) -> Result<()> {
        if message.from.is_none() {
            message.from = Some(self.from.clone());
        }
        self.inner.send(message).await
    }
}

/// Mailer that tries each inner mailer in order until one succeeds.
///
/// Mirrors Laravel's `failover` transport: the first mailer that delivers wins;
/// when every mailer fails, the last transport error is returned.
pub struct FailoverMailer {
    /// Member mailers, tried in order.
    mailers: Vec<Arc<dyn Mailer>>,
}

impl FailoverMailer {
    /// Create a failover mailer over `mailers`, tried in order.
    pub fn new(mailers: Vec<Arc<dyn Mailer>>) -> Self {
        Self { mailers }
    }

    /// The member mailers, in delivery order.
    pub fn mailers(&self) -> &[Arc<dyn Mailer>] {
        &self.mailers
    }
}

#[async_trait]
impl Mailer for FailoverMailer {
    /// Try each member mailer in order, returning the first success.
    async fn send(&self, message: MailMessage) -> Result<()> {
        let mut last_error = None;
        for mailer in &self.mailers {
            match mailer.send(message.clone()).await {
                Ok(()) => return Ok(()),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error
            .unwrap_or_else(|| MailError::Transport("failover mailer has no members".into())))
    }
}

/// In-memory mailer that records every delivered message.
///
/// Useful for tests and local development: no network is touched, and the
/// delivered messages can be asserted through [`ArrayMailer::sent`].
#[derive(Clone, Default)]
pub struct ArrayMailer {
    /// Messages delivered so far, in order.
    sent: Arc<Mutex<Vec<MailMessage>>>,
}

impl ArrayMailer {
    /// Create an empty recording mailer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of every delivered message, in order.
    pub fn sent(&self) -> Vec<MailMessage> {
        self.sent.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Number of messages delivered.
    pub fn count(&self) -> usize {
        self.sent.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// Most recently delivered message, if any.
    pub fn last(&self) -> Option<MailMessage> {
        self.sent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .last()
            .cloned()
    }

    /// Drop every recorded message.
    pub fn clear(&self) {
        self.sent.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
}

#[async_trait]
impl Mailer for ArrayMailer {
    /// Validate and record `message` in memory.
    async fn send(&self, message: MailMessage) -> Result<()> {
        message.validate()?;
        self.sent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(message);
        Ok(())
    }
}

/// Mailer that formats messages to a log sink instead of delivering them.
#[derive(Clone, Default)]
pub struct LogMailer {
    /// Formatted lines captured so far.
    lines: Arc<Mutex<Vec<String>>>,
    /// Whether captured lines are also printed to stdout.
    echo: bool,
}

impl LogMailer {
    /// Create a logging mailer that records formatted lines.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a logging mailer that also prints lines to stdout.
    pub fn with_echo(echo: bool) -> Self {
        Self {
            lines: Arc::default(),
            echo,
        }
    }

    /// Formatted log lines captured so far.
    pub fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Drop every captured line.
    pub fn clear(&self) {
        self.lines.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }

    /// Render `message` as a single diagnostic log line.
    pub fn format(message: &MailMessage) -> String {
        let recipients = message
            .recipients()
            .map(|address| address.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let body = message
            .html
            .as_deref()
            .or(message.text.as_deref())
            .unwrap_or("");
        format!(
            "mail to [{recipients}] subject={:?} bytes={}",
            message.subject,
            body.len()
        )
    }
}

#[async_trait]
impl Mailer for LogMailer {
    /// Validate, format, and capture `message` (optionally echoing it).
    async fn send(&self, message: MailMessage) -> Result<()> {
        message.validate()?;
        let line = Self::format(&message);
        if self.echo {
            println!("{line}");
        }
        self.lines
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(line);
        Ok(())
    }
}

/// Registry slot holding the process-wide mailer.
static MAILER: OnceLock<RwLock<Option<Arc<dyn Mailer>>>> = OnceLock::new();

/// Access the lazily-initialized mailer registry slot.
fn slot() -> &'static RwLock<Option<Arc<dyn Mailer>>> {
    MAILER.get_or_init(|| RwLock::new(None))
}

/// Install the process-wide mailer used by [`Mail`](crate::Mail) and queued
/// notifications.
pub fn set_mailer(mailer: Arc<dyn Mailer>) {
    *slot().write().unwrap_or_else(|p| p.into_inner()) = Some(mailer);
}

/// The process-wide mailer, when one has been installed.
pub fn mailer() -> Option<Arc<dyn Mailer>> {
    slot().read().unwrap_or_else(|p| p.into_inner()).clone()
}
