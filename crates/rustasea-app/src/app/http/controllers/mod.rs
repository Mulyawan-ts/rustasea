//! Application controllers.
//!
//! Every controller is a plain struct implementing
//! [`rustasea::http::Controller`], so it inherits the shared
//! `json`/`json_status`/`validation_error`/`redirect`/`see_other` helpers
//! instead of re-declaring them.

pub mod post_controller;

pub use post_controller::PostController;
