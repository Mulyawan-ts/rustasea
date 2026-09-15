//! CLI adapter — drive an [`Action`] from a console command.
//!
//! [`ActionCommand`] builds an action's input from either a JSON string or a
//! vector of console arguments (a single JSON argument). It runs the same
//! `validate → authorize → handle` pipeline and can either return the typed
//! output or render it as pretty JSON for printing.

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::{ActionError, Result};
use crate::Action;

/// A console invocation of an action.
pub struct ActionCommand<A: Action> {
    action: A,
    input: A::Input,
}

impl<A> ActionCommand<A>
where
    A: Action,
    A::Input: DeserializeOwned,
{
    /// Build a command from a raw JSON string for the action's input.
    pub fn from_json(action: A, json: &str) -> Result<Self> {
        let input = serde_json::from_str(json).map_err(ActionError::from)?;
        Ok(Self { action, input })
    }

    /// Build a command from console arguments (exactly one JSON argument).
    ///
    /// An empty argument list yields a [`ActionError::Serialization`] carrying a
    /// usage hint, so `action:run` fails with an actionable message rather than
    /// a bare parse error.
    pub fn from_args(action: A, args: &[String]) -> Result<Self> {
        let Some(json) = args.first() else {
            return Err(ActionError::Serialization(
                "expected one JSON argument, e.g. `{ name: \"Ada\" }`".to_string(),
            ));
        };
        Self::from_json(action, json)
    }

    /// Run the action, returning its typed output.
    pub async fn run(self) -> Result<A::Output> {
        self.action.run(self.input).await
    }

    /// Run the action and render its output as pretty-printed JSON.
    ///
    /// This is the "CLI prints result" path; a serialization failure of the
    /// output becomes an [`ActionError::Serialization`].
    pub async fn run_to_string(self) -> Result<String>
    where
        A::Output: Serialize,
    {
        let output = self.run().await?;
        serde_json::to_string_pretty(&output).map_err(ActionError::from)
    }
}
