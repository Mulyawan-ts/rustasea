//! Integration tests for the testing fakes (LARAVEL-015).
//!
//! Each fake gets a positive test (the intercepted side effect is recorded and
//! the assertion passes) and a negative test (an unmet expectation panics with a
//! descriptive message). The `#[should_panic(expected = ...)]` cases pin the
//! failure text so a regression that drops the "what was recorded" detail fails
//! loudly.
//!
//! Isolation note: the queue route registry and the process-wide mailer are
//! global. Every test here owns a distinct job type, connection, and queue (or a
//! private [`FakeMailer`] instance) so parallel execution cannot leak state.

use rustasea_mail::{Mail, MailAddress, Mailable};
use rustasea_queue::{Job, JobError, Queue};
use rustasea_testing::{FakeDispatcher, FakeMailer, FakeQueue};

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// A domain event used by the dispatcher-fake tests.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct UserRegistered {
    /// Id of the registered user.
    id: u64,
}

impl rustasea_events::Event for UserRegistered {
    /// Stable event name for diagnostics.
    fn event_name(&self) -> &'static str {
        "UserRegistered"
    }
}

/// A second event that is never dispatched in the negative test.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PasswordReset;

impl rustasea_events::Event for PasswordReset {}

/// Positive: a handler that dispatches an event is observable via the fake.
#[tokio::test]
async fn dispatcher_fake_records_dispatched_event() {
    let events = FakeDispatcher::new();

    events
        .dispatch(UserRegistered { id: 7 })
        .await
        .expect("record dispatch");

    events.assert_dispatched::<UserRegistered>();
    events.assert_dispatched_times::<UserRegistered>(1);
    events.assert_dispatched_where::<UserRegistered, _>(|e| e.id == 7);
    events.assert_not_dispatched::<PasswordReset>();

    let recorded = events.dispatched::<UserRegistered>();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].id, 7);
}

/// Negative: asserting an undispatched event panics and names what was recorded.
#[tokio::test]
#[should_panic(expected = "expected event `")]
async fn dispatcher_fake_panics_on_missing_event() {
    let events = FakeDispatcher::new();
    events
        .dispatch(UserRegistered { id: 1 })
        .await
        .expect("record dispatch");

    // `PasswordReset` was never dispatched: this must panic.
    events.assert_dispatched::<PasswordReset>();
}

/// Negative: `assert_not_dispatched` panics when the event *was* dispatched.
#[tokio::test]
#[should_panic(expected = "NOT to have been dispatched")]
async fn dispatcher_fake_panics_when_event_present() {
    let events = FakeDispatcher::new();
    events
        .dispatch(UserRegistered { id: 2 })
        .await
        .expect("record dispatch");

    events.assert_not_dispatched::<UserRegistered>();
}

// ---------------------------------------------------------------------------
// Queue
// ---------------------------------------------------------------------------

/// Declare a queue-test job: one `user_id` field and an empty `handle` body.
///
/// Every queue test instantiates its own job type. The route registry is keyed
/// by `std::any::type_name::<J>()` and is process-wide, so reusing one job type
/// across tests makes the harness's parallel run panic with `DuplicateRoute`
/// before any assertion is reached — the very failure this file's isolation
/// note promises cannot happen. A distinct type per test (plus a distinct
/// connection/queue name) keeps the route registry, the driver map, and each
/// [`FakeQueue`] buffer disjoint.
macro_rules! queue_test_job {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
        struct $name {
            /// Recipient id.
            user_id: u64,
        }

        #[rustasea_queue::async_trait]
        impl Job for $name {
            /// No-op body; the fake never executes jobs.
            async fn handle(self) -> std::result::Result<(), JobError> {
                Ok(())
            }
        }
    };
}

queue_test_job!(
    /// Positive test: the job that is dispatched and intercepted.
    InterceptedJob
);

queue_test_job!(
    /// Positive test: a control job that is never pushed.
    InterceptedUnpushedJob
);

queue_test_job!(
    /// Missing-job negative test: the job that is dispatched.
    MissingJob
);

queue_test_job!(
    /// Missing-job negative test: a control job that is never pushed.
    MissingUnpushedJob
);

queue_test_job!(
    /// Wrong-queue negative test: the job that is dispatched.
    WrongQueueJob
);

/// Positive: a real `Queue::dispatch` is intercepted by the installed fake.
#[tokio::test]
async fn queue_fake_intercepts_dispatch() {
    let queue = FakeQueue::install("fake-queue-positive");
    Queue::route::<InterceptedJob>("fake-queue-positive", "emails").expect("route");

    Queue::dispatch(InterceptedJob { user_id: 42 })
        .await
        .expect("dispatch intercepted");

    queue.assert_pushed::<InterceptedJob>();
    queue.assert_pushed_on::<InterceptedJob>("emails");
    queue.assert_pushed_times::<InterceptedJob>(1);
    queue.assert_not_pushed::<InterceptedUnpushedJob>();

    let pushed = queue.pushed_of::<InterceptedJob>();
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0].queue, "emails");
    assert_eq!(pushed[0].connection, "fake-queue-positive");
}

/// Negative: asserting an unpushed job panics and lists the recorded pushes.
#[tokio::test]
#[should_panic(expected = "expected job `")]
async fn queue_fake_panics_on_missing_job() {
    let queue = FakeQueue::install("fake-queue-missing");
    Queue::route::<MissingJob>("fake-queue-missing", "emails").expect("route");

    Queue::dispatch(MissingJob { user_id: 1 })
        .await
        .expect("dispatch intercepted");

    // `MissingUnpushedJob` was never pushed: this must panic.
    queue.assert_pushed::<MissingUnpushedJob>();
}

/// Negative: asserting the wrong queue panics even when the job was pushed.
#[tokio::test]
#[should_panic(expected = "onto queue `urgent`")]
async fn queue_fake_panics_on_wrong_queue() {
    let queue = FakeQueue::install("fake-queue-wrong-queue");
    Queue::route::<WrongQueueJob>("fake-queue-wrong-queue", "emails").expect("route");

    Queue::dispatch(WrongQueueJob { user_id: 2 })
        .await
        .expect("dispatch intercepted");

    queue.assert_pushed_on::<WrongQueueJob>("urgent");
}

// ---------------------------------------------------------------------------
// Mail
// ---------------------------------------------------------------------------

/// A mailable used by the mailer-fake tests.
struct WelcomeMail {
    /// Recipient address.
    to: MailAddress,
}

impl Mailable for WelcomeMail {
    /// Subject line.
    fn subject(&self) -> String {
        "Welcome aboard".to_string()
    }

    /// Primary recipients.
    fn to(&self) -> Vec<MailAddress> {
        vec![self.to.clone()]
    }

    /// HTML body.
    fn html_body(&self) -> String {
        "<h1>Welcome</h1>".to_string()
    }
}

/// A mailable that is never sent in the negative test.
struct InvoiceMail;

impl Mailable for InvoiceMail {
    /// Subject line.
    fn subject(&self) -> String {
        "Your invoice".to_string()
    }

    /// Primary recipients.
    fn to(&self) -> Vec<MailAddress> {
        vec![MailAddress::from_email("billing@example.com")]
    }

    /// HTML body.
    fn html_body(&self) -> String {
        "<p>Invoice</p>".to_string()
    }
}

/// Positive: a sent mailable is recorded with its type tag and recipients.
#[tokio::test]
async fn mailer_fake_records_sent_mailable() {
    let mail = FakeMailer::new();

    mail.send_mailable(&WelcomeMail {
        to: MailAddress::from_email("ada@example.com"),
    })
    .await
    .expect("record send");

    mail.assert_sent::<WelcomeMail>();
    mail.assert_sent_times::<WelcomeMail>(1);
    mail.assert_sent_to::<WelcomeMail>("ada@example.com");
    mail.assert_not_sent::<InvoiceMail>();
    mail.assert_sent_message("Welcome aboard");

    let sent = mail.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].subject, "Welcome aboard");
}

/// Positive: the raw `Mailer::send` path is intercepted via `Mail::send`.
#[tokio::test]
async fn mailer_fake_install_intercepts_mail_send() {
    let mail = FakeMailer::install();

    Mail::send(&WelcomeMail {
        to: MailAddress::from_email("grace@example.com"),
    })
    .await
    .expect("send intercepted");

    // The raw path erases the mailable type, so assert on the message.
    mail.assert_sent_message("Welcome aboard");
    assert_eq!(mail.count(), 1);
}

/// Negative: asserting an unsent mailable panics and lists what was sent.
#[tokio::test]
#[should_panic(expected = "expected mailable `")]
async fn mailer_fake_panics_on_missing_mailable() {
    let mail = FakeMailer::new();
    mail.send_mailable(&WelcomeMail {
        to: MailAddress::from_email("ada@example.com"),
    })
    .await
    .expect("record send");

    // `InvoiceMail` was never sent: this must panic.
    mail.assert_sent::<InvoiceMail>();
}

/// Negative: asserting the wrong recipient panics even when the mailable was sent.
#[tokio::test]
#[should_panic(expected = "to `nobody@example.com`")]
async fn mailer_fake_panics_on_wrong_recipient() {
    let mail = FakeMailer::new();
    mail.send_mailable(&WelcomeMail {
        to: MailAddress::from_email("ada@example.com"),
    })
    .await
    .expect("record send");

    mail.assert_sent_to::<WelcomeMail>("nobody@example.com");
}
