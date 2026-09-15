//! Queue adapter — run an [`Action`] from a worker.
//!
//! [`ActionJob`] is a serializable job carrying the action instance and its
//! input. Dispatching it through `rustasea-queue` moves the work off the
//! request path; the worker runs the identical `validate → authorize → handle`
//! pipeline. `handle` discards the action's output (the `Job` contract returns
//! `()`), so a caller that needs the result uses [`ActionJob::run`] directly or
//! the sync connection.

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::Action;

/// A serializable action invocation.
///
/// Build it with [`ActionJob::new`], then `job.dispatch().await` (through a
/// registered route) or [`Queue::dispatch_sync`](rustasea_queue::Queue::dispatch_sync)
/// for an inline run. Use [`ActionJob::run`] to obtain the action's typed output
/// without going through the queue.
#[derive(Debug, Clone)]
pub struct ActionJob<A>
where
    A: Action,
{
    /// The action instance to invoke.
    pub action: A,
    /// The action's input.
    pub input: A::Input,
}

impl<A> ActionJob<A>
where
    A: Action,
{
    /// Build a job invoking `action` with `input`.
    pub fn new(action: A, input: A::Input) -> Self {
        Self { action, input }
    }

    /// Run the action now, returning its typed output.
    ///
    /// This is the same pipeline the queue worker runs; it is public so a
    /// synchronous caller (or a test) can read the output the `Job` contract
    /// discards.
    pub async fn run(self) -> Result<A::Output> {
        self.action.run(self.input).await
    }
}

impl<A> Serialize for ActionJob<A>
where
    A: Action + Serialize,
    A::Input: Serialize,
{
    /// Serialize the action and its input as `{ "action": …, "input": … }`.
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("ActionJob", 2)?;
        state.serialize_field("action", &self.action)?;
        state.serialize_field("input", &self.input)?;
        state.end()
    }
}

impl<'de, A> Deserialize<'de> for ActionJob<A>
where
    A: Action + DeserializeOwned,
    A::Input: DeserializeOwned,
{
    /// Deserialize the action and its input.
    ///
    /// Hand-written because deriving `Deserialize` over an associated input type
    /// (`A::Input`) is ambiguous for serde's generated lifetime bound; both
    /// fields are reconstructed through their `DeserializeOwned` supertrait.
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            action: serde_json::Value,
            input: serde_json::Value,
        }
        let raw = Raw::deserialize(deserializer)?;
        let action = serde_json::from_value::<A>(raw.action).map_err(serde::de::Error::custom)?;
        let input =
            serde_json::from_value::<A::Input>(raw.input).map_err(serde::de::Error::custom)?;
        Ok(Self { action, input })
    }
}

#[async_trait]
impl<A> rustasea_queue::Job for ActionJob<A>
where
    A: Action + Serialize,
    A::Input: Serialize + Send + Sync + 'static,
{
    /// Run the action and discard its output.
    ///
    /// A domain/validation/authorization failure becomes a
    /// `JobError::Exception` so the worker retries then dead-letters the job.
    async fn handle(self) -> std::result::Result<(), rustasea_queue::JobError> {
        self.run()
            .await
            .map(|_| ())
            .map_err(|error| rustasea_queue::JobError::Exception(error.to_string()))
    }
}
