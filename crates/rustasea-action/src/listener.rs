//! Event adapter — run an [`Action`] in response to an [`Event`].
//!
//! [`ActionListener`] implements the events `Listener<E>` trait for any event
//! `E` whose payload converts into the action's input (`A::Input: From<E>`).
//! Register it with `Dispatcher::listen::<E, _>(ActionListener::new(action))`
//! and dispatch events as usual; the listener runs the identical
//! `validate → authorize → handle` pipeline and maps any failure onto the
//! events `EventError::Listener` variant.

use async_trait::async_trait;

use crate::error::ActionError;
use crate::Action;

/// A listener that runs an action when `E` is dispatched.
pub struct ActionListener<A: Action> {
    action: A,
}

impl<A: Action> ActionListener<A> {
    /// Wrap `action` in an event listener.
    pub fn new(action: A) -> Self {
        Self { action }
    }
}

#[async_trait]
impl<A, E> rustasea_events::Listener<E> for ActionListener<A>
where
    A: Action,
    E: rustasea_events::Event,
    A::Input: From<E>,
{
    /// Convert the event into the action input and run the action.
    ///
    /// The action's output is discarded (the listener contract returns `()`); a
    /// validation/authorization/domain failure surfaces as an
    /// `EventError::Listener` so a queue-backed listener job retries then
    /// dead-letters it.
    async fn handle(&self, event: E) -> std::result::Result<(), rustasea_events::EventError> {
        let input = A::Input::from(event);
        self.action
            .run(input)
            .await
            .map(|_| ())
            .map_err(|error| rustasea_events::EventError::Listener(render(error)))
    }
}

/// Render an [`ActionError`] into the events listener failure message.
///
/// Validation failures name the failing fields so an operator reading the
/// dead-letter trace can see *why* the action rejected the event payload.
fn render(error: ActionError) -> String {
    match error {
        ActionError::Validation(bag) => {
            let fields = bag.keys().cloned().collect::<Vec<_>>().join(", ");
            format!("action input failed validation for fields: {fields}")
        }
        other => other.to_string(),
    }
}
