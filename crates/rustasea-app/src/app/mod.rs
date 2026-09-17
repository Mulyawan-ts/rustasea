//! Example application domain - mirrors the `cargo rustasea new` scaffold layout.
//!
//! The generated application keeps its domain code under `app/`: HTTP
//! controllers and form requests, Eloquent-style models, and the shared
//! `Action` units of work. This module reproduces that layout inside the
//! runnable `rustasea-app` crate so the scaffold shape is visible in real,
//! compiling code rather than only in the scaffold templates.
//!
//! The `Post` resource is deliberately small but complete: a model with an
//! in-memory repository, a JSON CRUD controller, two validated form requests,
//! and a `CreatePostAction` that owns the write. The demo routes live in
//! `crate::routes::examples` and are served at `/examples/posts`.

pub mod actions;
pub mod http;
pub mod models;
