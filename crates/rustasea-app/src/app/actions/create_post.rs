//! Creates and persists a post.
//!
//! Creation is an [`Action`](rustasea::action::Action): the same unit of work is
//! reachable from the HTTP post controller, a queued job, the CLI, or an event
//! without duplicating the persistence logic. The action takes the validated
//! [`StorePostRequest`] as its input and returns the created [`Post`].

use rustasea::action::{async_trait, Action};
use rustasea::validation::{ErrorBag, Validatable};

use crate::app::http::requests::StorePostRequest;
use crate::app::models::{Post, PostRepository};

/// Domain error returned by [`CreatePostAction::handle`].
#[derive(Debug)]
pub enum CreatePostError {
    /// The row could not be persisted.
    Persistence(String),
}

impl std::fmt::Display for CreatePostError {
    /// Render the persistence failure.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Persistence(detail) => write!(formatter, "post persistence failed: {detail}"),
        }
    }
}

impl std::error::Error for CreatePostError {}

/// The create-post action.
pub struct CreatePostAction;

#[async_trait]
impl Action for CreatePostAction {
    type Input = StorePostRequest;
    type Output = Post;
    type Error = CreatePostError;

    /// Persist the post and return the stored row.
    ///
    /// The in-memory store cannot fail, but the action still enforces the
    /// "title must not be blank" invariant as a second line of defense so a
    /// non-HTTP adapter that bypasses [`Self::validate`] cannot persist an empty
    /// row.
    async fn handle(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        if input.title.trim().is_empty() {
            return Err(CreatePostError::Persistence(
                "title must not be blank".to_string(),
            ));
        }
        Ok(PostRepository::create(
            input.title,
            input.body,
            input.published,
        ))
    }

    /// Re-run the request's validation so a non-HTTP adapter validates too.
    fn validate(&self, input: &Self::Input) -> Result<(), ErrorBag> {
        input.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The action persists a post and echoes the submitted fields.
    #[tokio::test]
    async fn handle_persists_the_post() {
        let _guard = crate::app::models::post::test_lock().lock().await;
        PostRepository::reset();
        let input = StorePostRequest {
            title: "Action".to_string(),
            body: "Body".to_string(),
            published: true,
        };
        let post = CreatePostAction
            .run(input)
            .await
            .expect("a valid input must persist");
        assert_eq!(post.title, "Action");
        assert!(post.published);
        assert!(PostRepository::find(post.id).is_some());
    }

    /// The `validate` hook rejects a blank input before `handle` runs.
    #[tokio::test]
    async fn validate_rejects_a_blank_input() {
        let _guard = crate::app::models::post::test_lock().lock().await;
        PostRepository::reset();
        let input = StorePostRequest::default();
        let error = CreatePostAction
            .run(input)
            .await
            .expect_err("a blank input must be rejected");
        assert!(error.is_validation());
        assert!(PostRepository::list().is_empty());
    }

    /// The `handle` invariant rejects a blank title even when `validate` is
    /// bypassed, and the domain error renders a message.
    #[tokio::test]
    async fn handle_rejects_a_blank_title_without_validation() {
        let _guard = crate::app::models::post::test_lock().lock().await;
        PostRepository::reset();
        let input = StorePostRequest {
            title: "   ".to_string(),
            body: "Body".to_string(),
            published: false,
        };
        let error = CreatePostAction
            .handle(input)
            .await
            .expect_err("a blank title must be rejected by handle");
        assert_eq!(
            error.to_string(),
            "post persistence failed: title must not be blank"
        );
        assert!(PostRepository::list().is_empty());
    }
}
