//! RustaSea Action pattern (ADOPT-028) — one unit of work across every entry
//! point.
//!
//! An [`Action`] is a single, self-contained unit of work with a typed input,
//! output, and error, plus optional `validate` and `authorize` hooks. The same
//! action instance is driven by four adapters that all run the identical
//! `validate → authorize → handle` pipeline:
//!
//! - [`ActionController`] — HTTP: parse the JSON body, map failures onto
//!   `400`/`422`/`403`/`500` (feature `http`).
//! - [`ActionJob`] — queue: a serializable job that runs the action in a worker
//!   (feature `queue`).
//! - [`ActionCommand`] — CLI: build the input from a JSON argument and print the
//!   pretty-printed output.
//! - [`ActionListener`] — events: run the action in response to an event whose
//!   payload converts into the action's input (feature `events`).
//!
//! This mirrors `lorisleiva/laravel-actions`: write the domain logic once, then
//! expose it over HTTP, the queue, the console, and the event bus without
//! duplicating validation or authorization.
//!
//! # Order of hooks
//!
//! Every adapter runs **validate first, then authorize, then handle**. A
//! validation failure therefore wins over an authorization failure when both
//! would fire (the 422 is returned, not the 403).

#![warn(missing_docs)]

mod error;

#[cfg(feature = "http")]
mod controller;
#[cfg(feature = "queue")]
mod job;
#[cfg(feature = "events")]
mod listener;

mod command;

pub use error::{ActionError, Result};

#[cfg(feature = "http")]
pub use controller::{ActionController, HttpOutcome};
#[cfg(feature = "queue")]
pub use job::ActionJob;
#[cfg(feature = "events")]
pub use listener::ActionListener;

pub use command::ActionCommand;

/// Re-exported so a generated action can `use rustasea::action::async_trait;`
/// without declaring its own `async-trait` dependency.
pub use async_trait::async_trait;

/// A single unit of work with a typed input/output/error and optional hooks.
///
/// Implementors provide `handle`; the `validate` and `authorize` hooks default
/// to allow. All four adapters run the same hook order (validate → authorize →
/// handle) so behaviour is identical across transports.
#[async_trait::async_trait]
pub trait Action: Send + Sync + 'static {
    /// The action's input (validated by `validate`, deserialized by adapters).
    type Input: Send;

    /// The action's successful output.
    type Output: Send;

    /// The domain error returned by `handle` on failure.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Execute the action body.
    async fn handle(&self, input: Self::Input) -> std::result::Result<Self::Output, Self::Error>;

    /// Validation hook; `Err` short-circuits before `handle` in every adapter.
    fn validate(
        &self,
        _input: &Self::Input,
    ) -> std::result::Result<(), rustasea_validation::ErrorBag> {
        Ok(())
    }

    /// Authorization hook; `false` → 403 (HTTP) / Unauthorized (other adapters).
    fn authorize(&self, _input: &Self::Input) -> bool {
        true
    }

    /// Run the full pipeline (`validate → authorize → handle`) once.
    ///
    /// This is the single execution gate shared by every adapter; implementors
    /// never override it. Domain errors are rendered to strings because an
    /// adapter (queue/CLI/event) has no channel to carry `Self::Error` — only
    /// the typed [`ActionError`] crosses the adapter boundary.
    async fn run(&self, input: Self::Input) -> Result<Self::Output> {
        self.validate(&input).map_err(ActionError::Validation)?;
        if !self.authorize(&input) {
            return Err(ActionError::Unauthorized);
        }
        self.handle(input)
            .await
            .map_err(|error| ActionError::Failed(error.to_string()))
    }
}
