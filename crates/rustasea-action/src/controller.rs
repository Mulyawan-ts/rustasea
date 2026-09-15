//! HTTP adapter — drive an [`Action`] from a JSON request body.
//!
//! [`ActionController`] wraps an action and exposes two entry points: `invoke`
//! for an already-typed input, and `invoke_json` which parses the raw request
//! body first. Both return an [`HttpOutcome`] that renders to an
//! `axum::response::Response` with the documented status/body mapping:
//!
//! | Failure | Status | Body |
//! | --- | --- | --- |
//! | body is not valid JSON | `400` | `{"message": …}` |
//! | `validate` rejects the input | `422` | `ErrorBag::into_json_body()` |
//! | `authorize` returns `false` | `403` | `{"message": …}` |
//! | `handle` fails | `500` | `{"message": "An unexpected error occurred."}` |
//! | output serialization fails | `500` | `{"message": "An unexpected error occurred."}` |
//! | success | `200` | serialized output |
//!
//! The `500` bodies are deliberately generic: a domain or serialization error
//! string can name internal types or data, so it is never echoed to the client
//! (matching `rustasea-http`'s prod-safe `5xx` rendering).
//!
//! # Route wiring
//!
//! ```ignore
//! use rustasea::action::{Action, ActionController};
//!
//! async fn publish(body: axum::body::Bytes) -> axum::response::Response {
//!     ActionController::new(PublishPost).invoke_json(&body).await.into_response()
//! }
//! ```

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::ActionError;
use crate::Action;

/// Client-facing body for every `500` (prod-safe).
///
/// Matches `rustasea-http`'s `AppError::client_detail` for `5xx`: the real
/// domain error string is never sent to the client, only this generic message.
const INTERNAL_ERROR_MESSAGE: &str = "An unexpected error occurred.";

/// The rendered status + JSON body of an action invocation.
#[derive(Debug, Clone)]
pub struct HttpOutcome {
    /// HTTP status the adapter selected.
    pub status: axum::http::StatusCode,
    /// JSON body (an object on success and on error).
    pub body: serde_json::Value,
}

impl HttpOutcome {
    /// Build an outcome with an explicit status and JSON body.
    pub fn new(status: axum::http::StatusCode, body: serde_json::Value) -> Self {
        Self { status, body }
    }

    /// Render into an `axum::response::Response` with a JSON content type.
    pub fn into_response(self) -> axum::response::Response {
        use axum::response::IntoResponse;
        (self.status, axum::Json(self.body)).into_response()
    }
}

/// HTTP entry point for an action.
///
/// The controller owns the action instance; register a handler that delegates
/// to [`ActionController::invoke_json`] (or `invoke`) and calls
/// [`HttpOutcome::into_response`].
pub struct ActionController<A: Action> {
    action: A,
}

impl<A: Action> ActionController<A> {
    /// Wrap `action` in an HTTP controller.
    pub fn new(action: A) -> Self {
        Self { action }
    }

    /// Invoke the action with an already-deserialized `input`.
    pub async fn invoke(&self, input: A::Input) -> HttpOutcome
    where
        A::Input: DeserializeOwned,
        A::Output: Serialize,
    {
        match self.action.run(input).await {
            Ok(output) => match serde_json::to_value(output) {
                Ok(body) => HttpOutcome::new(axum::http::StatusCode::OK, body),
                // The serialization error can name internal types/fields, so the
                // body stays generic; the status is kept as a `500`.
                Err(_error) => HttpOutcome::new(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({ "message": INTERNAL_ERROR_MESSAGE }),
                ),
            },
            Err(error) => Self::outcome_for(error),
        }
    }

    /// Parse `body` as the action's input JSON, then invoke the action.
    ///
    /// A body that is not valid JSON for `A::Input` becomes a `400`; the
    /// `validate`/`authorize`/`handle` pipeline then runs as in [`Self::invoke`].
    pub async fn invoke_json(&self, body: &[u8]) -> HttpOutcome
    where
        A::Input: DeserializeOwned,
        A::Output: Serialize,
    {
        let input: A::Input = match serde_json::from_slice(body) {
            Ok(input) => input,
            Err(error) => {
                return HttpOutcome::new(
                    axum::http::StatusCode::BAD_REQUEST,
                    serde_json::json!({ "message": format!("invalid JSON body: {error}") }),
                );
            }
        };
        self.invoke(input).await
    }

    /// Map an [`ActionError`] onto its documented status/body.
    fn outcome_for(error: ActionError) -> HttpOutcome {
        use axum::http::StatusCode;
        match error {
            ActionError::Validation(bag) => {
                HttpOutcome::new(StatusCode::UNPROCESSABLE_ENTITY, bag.into_json_body())
            }
            ActionError::Unauthorized => HttpOutcome::new(
                StatusCode::FORBIDDEN,
                serde_json::json!({ "message": "This action is unauthorized." }),
            ),
            ActionError::Serialization(message) => HttpOutcome::new(
                StatusCode::BAD_REQUEST,
                serde_json::json!({ "message": message }),
            ),
            // The domain error string is server-side detail; never echo it.
            ActionError::Failed(_message) => HttpOutcome::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({ "message": INTERNAL_ERROR_MESSAGE }),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    /// Fails `handle` with a domain error whose text must never reach the client.
    struct FailingAction;

    #[async_trait::async_trait]
    impl Action for FailingAction {
        type Input = u32;
        type Output = u32;
        type Error = std::io::Error;

        async fn handle(
            &self,
            _input: Self::Input,
        ) -> std::result::Result<Self::Output, Self::Error> {
            Err(std::io::Error::other("secret db detail"))
        }
    }

    /// A handle failure yields a generic `500`, never the domain error string.
    #[tokio::test]
    async fn handle_failure_is_sanitized_to_a_generic_500() {
        let outcome = ActionController::new(FailingAction).invoke(1).await;
        assert_eq!(outcome.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            outcome.body,
            serde_json::json!({ "message": "An unexpected error occurred." })
        );
        assert!(
            !outcome.body.to_string().contains("secret db detail"),
            "the domain error text must not leak into the 500 body"
        );
    }

    /// An output whose `Serialize` always fails (models an unserializable value).
    struct UnserializableOutput;

    impl Serialize for UnserializableOutput {
        fn serialize<S>(&self, _serializer: S) -> std::result::Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            Err(serde::ser::Error::custom("secret serialization detail"))
        }
    }

    struct UnserializableAction;

    #[async_trait::async_trait]
    impl Action for UnserializableAction {
        type Input = u32;
        type Output = UnserializableOutput;
        type Error = std::convert::Infallible;

        async fn handle(
            &self,
            _input: Self::Input,
        ) -> std::result::Result<Self::Output, Self::Error> {
            Ok(UnserializableOutput)
        }
    }

    /// A serialization failure keeps its `500` status but gets a generic body.
    #[tokio::test]
    async fn output_serialization_failure_is_sanitized_to_a_generic_500() {
        let outcome = ActionController::new(UnserializableAction).invoke(1).await;
        assert_eq!(outcome.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            outcome.body,
            serde_json::json!({ "message": "An unexpected error occurred." })
        );
        assert!(
            !outcome
                .body
                .to_string()
                .contains("secret serialization detail"),
            "the serialization error text must not leak into the 500 body"
        );
    }
}
