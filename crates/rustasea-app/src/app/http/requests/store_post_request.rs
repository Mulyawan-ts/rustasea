//! Validated create-post input.
//!
//! Built against the real validation contract: the payload derives
//! `serde::Deserialize` (required by `Validatable: DeserializeOwned`) and
//! `serde::Serialize` (used to build the JSON value the rules run against), and
//! `Validatable::validate` builds a `Rules` set with the fluent
//! `Rules::field(field, "rule|rule")` grammar.

use rustasea::validation::{ErrorBag, Rules, Validatable};

/// Create-post form request.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct StorePostRequest {
    /// Post headline.
    pub title: String,
    /// Post body text.
    pub body: String,
    /// Whether the post is visible immediately; optional, defaults to `false`.
    #[serde(default)]
    pub published: bool,
}

impl Validatable for StorePostRequest {
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
        let request = StorePostRequest {
            title: "Hello".to_string(),
            body: "World".to_string(),
            published: false,
        };
        assert!(request.validate().is_ok());
    }

    /// A blank title and body each produce a field error.
    #[test]
    fn blank_fields_are_rejected() {
        let request = StorePostRequest::default();
        let bag = request.validate().expect_err("blank fields must fail");
        assert!(!bag.get("title").is_empty());
        assert!(!bag.get("body").is_empty());
    }

    /// An over-long title is rejected by `max:255`.
    #[test]
    fn over_long_title_is_rejected() {
        let request = StorePostRequest {
            title: "x".repeat(256),
            body: "ok".to_string(),
            published: false,
        };
        let bag = request
            .validate()
            .expect_err("an over-long title must fail");
        assert!(!bag.get("title").is_empty());
    }
}
