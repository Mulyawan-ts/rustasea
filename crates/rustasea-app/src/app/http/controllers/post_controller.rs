//! JSON CRUD controller for the example `posts` resource.
//!
//! [`PostController`] implements [`rustasea::http::Controller`], so the shared
//! `json`/`json_status`/`validation_error` helpers are inherited rather than
//! re-declared. Every handler is a real associated function (no `todo!`): the
//! routes in `crate::routes::examples` bind them directly.
//!
//! `store` delegates the write to
//! [`CreatePostAction`](crate::app::actions::create_post::CreatePostAction) so
//! the same unit of work is reachable from HTTP, the queue, the CLI, and the
//! event bus; the other handlers call the repository directly.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use uuid::Uuid;

use rustasea::action::{Action, ActionError};
use rustasea::http::Controller;
use rustasea::validation::Validatable;

use crate::app::actions::create_post::CreatePostAction;
use crate::app::http::requests::{StorePostRequest, UpdatePostRequest};
use crate::app::models::PostRepository;

/// Controller for the `/examples/posts` resource.
pub struct PostController;

impl Controller for PostController {}

impl PostController {
    /// `GET /examples/posts` - list every post as a JSON array.
    pub async fn index() -> Response {
        Self::json(PostRepository::list())
    }

    /// `GET /examples/posts/{id}` - show one post or answer `404`.
    pub async fn show(Path(id): Path<Uuid>) -> Response {
        match PostRepository::find(id) {
            Some(post) => Self::json(post),
            None => not_found(id),
        }
    }

    /// `POST /examples/posts` - create a post through [`CreatePostAction`].
    ///
    /// The action's `validate` hook runs the [`StorePostRequest`] rules, so a
    /// bad payload answers `422` with the documented error envelope and a good
    /// one answers `201` with the stored post.
    pub async fn store(Json(payload): Json<Value>) -> Response {
        let request = match StorePostRequest::validate_value(&payload) {
            Ok(request) => request,
            Err(bag) => return Self::validation_error(bag.into_json_body()),
        };
        match CreatePostAction.run(request).await {
            Ok(post) => Self::json_status(StatusCode::CREATED, post),
            Err(error) => action_error(error),
        }
    }

    /// `PUT`/`PATCH /examples/posts/{id}` - update a post or answer `404`.
    pub async fn update(Path(id): Path<Uuid>, Json(payload): Json<Value>) -> Response {
        let request = match UpdatePostRequest::validate_value(&payload) {
            Ok(request) => request,
            Err(bag) => return Self::validation_error(bag.into_json_body()),
        };
        match PostRepository::update(id, request.title, request.body, request.published) {
            Some(post) => Self::json(post),
            None => not_found(id),
        }
    }

    /// `DELETE /examples/posts/{id}` - delete a post or answer `404`.
    pub async fn destroy(Path(id): Path<Uuid>) -> Response {
        if PostRepository::delete(id) {
            Self::json(json!({ "deleted": true, "id": id }))
        } else {
            not_found(id)
        }
    }
}

/// Build the `404` JSON envelope for an unknown post id.
fn not_found(id: Uuid) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "message": format!("No post with id {id}.") })),
    )
        .into_response()
}

/// Map an [`ActionError`] onto the documented status/body.
///
/// A validation failure keeps its `422` envelope; every other failure becomes a
/// generic `500` so internal detail is never echoed to the client.
fn action_error(error: ActionError) -> Response {
    match error {
        ActionError::Validation(bag) => PostController::validation_error(bag.into_json_body()),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "message": "An unexpected error occurred." })),
        )
            .into_response(),
    }
}
