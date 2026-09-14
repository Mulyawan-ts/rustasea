//! `FakeMailer` — the `Mail::fake()` equivalent.
//!
//! [`FakeMailer`] implements [`rustasea_mail::Mailer`], so it installs through
//! the real [`rustasea_mail::set_mailer`] seam (see [`FakeMailer::install`]) and
//! intercepts `Mail::send` with no call-site change. Because `Mailer::send`
//! receives an already-built [`rustasea_mail::MailMessage`], the concrete
//! mailable type is erased on that path — the type-tagged assertions match the
//! tag captured by [`FakeMailer::send_mailable`].

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use rustasea_mail::{MailMessage, Mailable, Mailer, Result as MailResult};

/// A single intercepted mail, with an optional mailable-type tag.
struct RecordedMail {
    /// The built message that was "delivered".
    message: MailMessage,
    /// Type name of the originating [`Mailable`], when sent via
    /// [`FakeMailer::send_mailable`]; `None` on the raw [`Mailer::send`] path.
    mailable: Option<&'static str>,
}

/// Recording mailer — the `Mail::fake()` equivalent.
///
/// Implements [`Mailer`] so it installs through [`rustasea_mail::set_mailer`]
/// (see [`FakeMailer::install`]); every `send` is recorded in memory. Because
/// `Mailer::send` receives an already-built [`MailMessage`], the concrete
/// [`Mailable`] type is erased there — [`assert_sent`](FakeMailer::assert_sent)
/// and [`assert_sent_to`](FakeMailer::assert_sent_to) match on the type tag
/// captured by [`send_mailable`](FakeMailer::send_mailable), while
/// [`assert_sent_message`](FakeMailer::assert_sent_message) covers the raw path.
///
/// ```no_run
/// use rustasea_testing::FakeMailer;
/// # use rustasea_mail::{MailAddress, Mailable};
/// # struct Welcome;
/// # impl Mailable for Welcome {
/// #     fn subject(&self) -> String { "Welcome".into() }
/// #     fn to(&self) -> Vec<MailAddress> { vec![MailAddress::from_email("ada@example.com")] }
/// #     fn html_body(&self) -> String { "<h1>Hi</h1>".into() }
/// # }
/// # async fn run() -> rustasea_mail::Result<()> {
/// let mail = FakeMailer::new();
/// mail.send_mailable(&Welcome).await?;
/// mail.assert_sent::<Welcome>();
/// mail.assert_sent_to::<Welcome>("ada@example.com");
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Default)]
pub struct FakeMailer {
    /// Intercepted messages, in send order.
    sent: Arc<Mutex<Vec<RecordedMail>>>,
}

impl FakeMailer {
    /// Create an empty recording mailer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a recording mailer and install it process-wide.
    ///
    /// Returns the shared handle so the test can assert against the same
    /// instance `Mail::send` records into. Note the process-wide mailer is
    /// shared state: install one fake per test binary to avoid cross-test
    /// interference.
    pub fn install() -> Arc<Self> {
        let fake = Arc::new(Self::default());
        rustasea_mail::set_mailer(fake.clone());
        fake
    }

    /// Build `mailable` and record it with its type tag.
    ///
    /// This is the type-preserving path: `assert_sent::<M>()` and
    /// `assert_sent_to::<M>(..)` match against the tag recorded here.
    pub async fn send_mailable<M: Mailable>(&self, mailable: &M) -> MailResult<()> {
        let message = mailable.build();
        self.record(message, Some(std::any::type_name::<M>()));
        Ok(())
    }

    /// Snapshot of every intercepted message, in send order.
    pub fn sent(&self) -> Vec<MailMessage> {
        let guard = self.sent.lock().unwrap_or_else(|p| p.into_inner());
        guard.iter().map(|r| r.message.clone()).collect()
    }

    /// Number of intercepted messages.
    pub fn count(&self) -> usize {
        self.sent.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// Assert a mailable of type `M` was sent.
    ///
    /// Matches on the type tag captured by
    /// [`send_mailable`](FakeMailer::send_mailable); messages recorded through
    /// the raw [`Mailer::send`] path carry no tag and are asserted with
    /// [`assert_sent_message`](FakeMailer::assert_sent_message).
    ///
    /// # Panics
    ///
    /// Panics with a message listing the recorded mailable types/subjects when
    /// no match was found.
    pub fn assert_sent<M: Mailable>(&self) {
        if self.sent_matching::<M>().next().is_none() {
            panic!(
                "expected mailable `{}` to have been sent, but it was not.\nrecorded sends: [{}]",
                std::any::type_name::<M>(),
                self.recorded_summary()
            );
        }
    }

    /// Assert a mailable of type `M` was **not** sent.
    ///
    /// # Panics
    ///
    /// Panics naming the send count when a matching message was recorded.
    pub fn assert_not_sent<M: Mailable>(&self) {
        let times = self.sent_matching::<M>().count();
        if times > 0 {
            panic!(
                "expected mailable `{}` NOT to have been sent, but it was sent {times} time(s).",
                std::any::type_name::<M>()
            );
        }
    }

    /// Assert a mailable of type `M` was sent exactly `times` times.
    ///
    /// # Panics
    ///
    /// Panics when the recorded count differs from `times`.
    pub fn assert_sent_times<M: Mailable>(&self, times: usize) {
        let actual = self.sent_matching::<M>().count();
        assert!(
            actual == times,
            "expected mailable `{}` to have been sent {times} time(s), but it was sent {actual} time(s).\nrecorded sends: [{}]",
            std::any::type_name::<M>(),
            self.recorded_summary()
        );
    }

    /// Assert a mailable of type `M` was sent to `email`.
    ///
    /// `email` is matched against the envelope recipients (`to` + `cc` + `bcc`).
    ///
    /// # Panics
    ///
    /// Panics with a message listing the recorded mailable types/recipients
    /// when no match was found.
    pub fn assert_sent_to<M: Mailable>(&self, email: &str) {
        let key = std::any::type_name::<M>();
        let matched = self
            .sent_matching::<M>()
            .any(|m| m.recipients().any(|address| address.email == email));
        if !matched {
            panic!(
                "expected mailable `{key}` to have been sent to `{email}`, but it was not.\nrecorded sends: [{}]",
                self.recorded_summary()
            );
        }
    }

    /// Assert a message with the given `subject` was sent (any mailable type).
    ///
    /// Use this for messages intercepted through the raw [`Mailer::send`] path,
    /// which cannot carry a mailable type tag.
    ///
    /// # Panics
    ///
    /// Panics listing the recorded subjects when no match was found.
    pub fn assert_sent_message(&self, subject: &str) {
        let guard = self.sent.lock().unwrap_or_else(|p| p.into_inner());
        if !guard.iter().any(|r| r.message.subject == subject) {
            panic!(
                "expected a mail message with subject `{subject}` to have been sent, but none was.\nrecorded sends: [{}]",
                self.recorded_summary()
            );
        }
    }

    /// Assert nothing at all was sent.
    ///
    /// # Panics
    ///
    /// Panics listing the recorded sends when any message was recorded.
    pub fn assert_nothing_sent(&self) {
        let count = self.count();
        assert!(
            count == 0,
            "expected no mail to have been sent, but {count} message(s) were.\nrecorded sends: [{}]",
            self.recorded_summary()
        );
    }

    /// Drop every recorded message.
    pub fn clear(&self) {
        self.sent.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }

    /// Record `message` with an optional mailable-type tag.
    fn record(&self, message: MailMessage, mailable: Option<&'static str>) {
        self.sent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(RecordedMail { message, mailable });
    }

    /// Recorded messages whose mailable type tag equals `M`'s type name.
    fn sent_matching<M: Mailable>(&self) -> impl Iterator<Item = MailMessage> {
        let key = std::any::type_name::<M>();
        let guard = self.sent.lock().unwrap_or_else(|p| p.into_inner());
        let matched: Vec<MailMessage> = guard
            .iter()
            .filter(|r| r.mailable == Some(key))
            .map(|r| r.message.clone())
            .collect();
        matched.into_iter()
    }

    /// Comma-separated list of recorded sends, for descriptive failures.
    fn recorded_summary(&self) -> String {
        let guard = self.sent.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .iter()
            .map(|r| {
                let to = r
                    .message
                    .recipients()
                    .map(|a| a.email.clone())
                    .collect::<Vec<_>>()
                    .join("+");
                format!(
                    "{}:{}->{}",
                    r.mailable.unwrap_or("<raw>"),
                    r.message.subject,
                    to
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[async_trait]
impl Mailer for FakeMailer {
    /// Record the built message (no mailable-type tag on this path).
    async fn send(&self, message: MailMessage) -> MailResult<()> {
        self.record(message, None);
        Ok(())
    }
}
