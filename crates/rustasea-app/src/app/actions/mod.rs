//! Domain actions - one responsibility per module.
//!
//! An action is a single unit of work reachable from HTTP, the queue, the CLI,
//! or an event without duplicating its logic (ADOPT-028). The example keeps one
//! action, [`create_post::CreatePostAction`], which the post controller's
//! `store` handler invokes.

pub mod create_post;
