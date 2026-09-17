//! Eloquent-style application models.
//!
//! Each model pairs a plain struct with a hand-written [`rustasea::orm::Model`]
//! impl, exactly as the scaffold's `app/models/*.rs` files do.

pub mod post;

pub use post::{Post, PostRepository};
