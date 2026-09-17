//! Form request objects (validation at the HTTP boundary).
//!
//! Each request implements [`rustasea::validation::Validatable`], so the
//! controller validates the payload once and maps a failure onto the documented
//! `422` error envelope.

pub mod store_post_request;
pub mod update_post_request;

pub use store_post_request::StorePostRequest;
pub use update_post_request::UpdatePostRequest;
