//! Testing fakes — intercept Mail, Queue, and Event side effects (Laravel parity).
//!
//! Laravel's `Illuminate\Support\Testing\Fakes` lets a test swap the real
//! infrastructure for a recording double (`Event::fake()`, `Queue::fake()`,
//! `Mail::fake()`, `Cache::fake()`) and then assert on what the application
//! *would* have done, without touching a network, database, or worker. This
//! module provides the rustasea equivalents: [`FakeDispatcher`], [`FakeQueue`],
//! [`FakeMailer`], and [`FakeCache`].
//!
//! Every fake records the intercepted items in order and exposes `assert_*`
//! helpers whose failure messages list what *was* recorded, so a broken
//! expectation points straight at the actual side effects.
//!
//! # Wiring each fake (substitution seams)
//!
//! The three real components expose different seams, so each fake installs
//! differently. No deep framework changes are made — each fake uses the closest
//! existing API.
//!
//! - **Queue** — [`rustasea_queue::Queue::register_driver`] is the seam.
//!   [`FakeQueue::install`] registers a recording [`rustasea_queue::QueueDriver`]
//!   under a named connection; route jobs to that connection (or
//!   `on_connection`) and the fake intercepts every `push` instead of a real
//!   driver. This is a true swap: `dispatch` code is unchanged.
//! - **Mail** — [`rustasea_mail::set_mailer`] is the seam.
//!   [`FakeMailer::install`] installs a recording [`rustasea_mail::Mailer`]
//!   process-wide, so `Mail::send` is intercepted with no call-site change.
//!   Because `Mailer::send` receives an already-built
//!   [`rustasea_mail::MailMessage`], the concrete mailable type is erased on
//!   that path; use [`FakeMailer::send_mailable`] when you want
//!   `assert_sent::<M>()` to match on the mailable type.
//! - **Events** — [`rustasea_events::Dispatcher`] is a zero-sized facade over a
//!   process-global listener map with **no installable seam** (static methods,
//!   no trait). [`FakeDispatcher`] therefore mirrors the `Dispatcher::dispatch`
//!   call shape and is injected by *calling it in place of* `Dispatcher` — a
//!   handler written against `&FakeDispatcher` records instead of fanning out to
//!   listeners. See [`FakeDispatcher`]'s docs for the documented pattern.
//!
//! ```no_run
//! use rustasea_testing::{FakeDispatcher, FakeMailer, FakeQueue};
//!
//! # async fn run() {
//! let dispatcher = FakeDispatcher::new();
//! // handler(&dispatcher) { dispatcher.dispatch(MyEvent { .. }).await? }
//! # }
//! ```

mod cache;
mod dispatcher;
mod mailer;
mod queue;

pub use cache::{FakeCache, Op, RecordedOp};
pub use dispatcher::FakeDispatcher;
pub use mailer::FakeMailer;
pub use queue::FakeQueue;
