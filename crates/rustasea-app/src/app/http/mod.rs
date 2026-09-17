//! Application HTTP layer - controllers and validated form requests.
//!
//! Mirrors the scaffold's `app/http/` tree: `controllers/` holds the request
//! handlers and `requests/` holds the validated input objects the handlers
//! consume.

pub mod controllers;
pub mod requests;
