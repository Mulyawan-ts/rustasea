//! Validated update-post input.
//!
//! Same contract as [`super::store_post_request::StorePostRequest`]; the field
//! set is identical because the example models a full replace on update.

use rustasea::validation::{ErrorBag, Rules, Validatable};

/// Update-post form request.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct UpdatePostRequest {
    /// Post headline.
    pub title: String,
    /// Post body text.
    pub body: String,
    /// Whether the post is visible to readers.
    #[serde(default)]
    pub published: bool,
}

impl Validatable for UpdatePostRequest {
    /// Validate `title` and `body` with the implemented rule grammar.
    fn validate(&self) -> Result<(), ErrorBag> {
        let rules = Rules::new()
            .field("title", "required|string|min:1|max:255")
            .field("body", "required|string|min:1");
        let data = rustasea::validation::serde_json::to_value(self)
            .map_err(|error| ErrorBag::from_message(error.to_string()))?;
        rules.validate(&data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A well-formed payload validates cleanly.
    #[test]
    fn valid_payload_passes() {
        let request = UpdatePostRequest {
            title: "Hello".to_string(),
            body: "World".to_string(),
            published: true,
        };
        assert!(request.validate().is_ok());
    }

    /// A blank title is rejected.
    #[test]
    fn blank_title_is_rejected() {
        let request = UpdatePostRequest {
            title: String::new(),
            body: "World".to_string(),
            published: false,
        };
        let bag = request.validate().expect_err("a blank title must fail");
        assert!(!bag.get("title").is_empty());
    }
}
