//! Unique-value registry backing [`Faker::unique_email`] and friends.
//!
//! FakerPHP's `unique()` modifier retries a generator until it produces a value
//! that has not been seen yet, giving up after a bounded number of attempts.
//! [`UniqueRegistry`] stores the seen values per field name and reports whether
//! a freshly drawn value is new, so the [`Faker`] facade can implement the
//! retry loop and surface exhaustion as
//! [`FakerError::UniqueExhausted`](super::FakerError::UniqueExhausted).
//!
//! [`Faker::unique_email`]: super::Faker::unique_email

use std::collections::{HashMap, HashSet};

/// Default number of draws attempted before a unique value is declared exhausted.
pub const DEFAULT_MAX_ATTEMPTS: usize = 1000;

/// Tracks previously emitted values so a faker can avoid repeating them.
#[derive(Debug, Clone)]
pub struct UniqueRegistry {
    /// Seen values keyed by the logical field name (`"email"`, `"name"`, …).
    seen: HashMap<&'static str, HashSet<String>>,
    /// Maximum draws attempted before declaring a field exhausted.
    max_attempts: usize,
}

impl Default for UniqueRegistry {
    /// A registry with [`DEFAULT_MAX_ATTEMPTS`] and no recorded values.
    fn default() -> Self {
        Self::new()
    }
}

impl UniqueRegistry {
    /// A fresh registry using the default attempt budget.
    pub fn new() -> Self {
        Self {
            seen: HashMap::new(),
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        }
    }

    /// A fresh registry with a custom attempt budget.
    pub fn with_max_attempts(max_attempts: usize) -> Self {
        Self {
            seen: HashMap::new(),
            max_attempts,
        }
    }

    /// The configured attempt budget.
    pub fn max_attempts(&self) -> usize {
        self.max_attempts
    }

    /// Record `value` for `field`, returning `true` when it is new.
    ///
    /// A duplicate returns `false` and leaves the registry unchanged, so the
    /// caller can redraw and try again.
    pub fn accept(&mut self, field: &'static str, value: String) -> bool {
        self.seen.entry(field).or_default().insert(value)
    }

    /// Forget every recorded value across all fields.
    pub fn reset(&mut self) {
        self.seen.clear();
    }

    /// Forget the recorded values for a single `field`.
    pub fn reset_field(&mut self, field: &'static str) {
        self.seen.remove(field);
    }

    /// How many distinct values have been recorded for `field`.
    pub fn len(&self, field: &'static str) -> usize {
        self.seen.get(field).map_or(0, HashSet::len)
    }

    /// Whether `field` has recorded no values yet.
    pub fn is_empty(&self, field: &'static str) -> bool {
        self.len(field) == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repeated value is rejected; a novel value is accepted.
    #[test]
    fn duplicate_values_are_rejected() {
        let mut registry = UniqueRegistry::new();
        assert!(registry.accept("email", "a@example.com".to_string()));
        assert!(!registry.accept("email", "a@example.com".to_string()));
        assert!(registry.accept("email", "b@example.com".to_string()));
        assert_eq!(registry.len("email"), 2);
    }

    /// Fields are tracked independently.
    #[test]
    fn fields_are_isolated() {
        let mut registry = UniqueRegistry::new();
        assert!(registry.accept("email", "same".to_string()));
        assert!(registry.accept("name", "same".to_string()));
        assert!(registry.is_empty("username"));
    }

    /// `reset` clears every field; `reset_field` clears exactly one.
    #[test]
    fn reset_clears_recorded_values() {
        let mut registry = UniqueRegistry::new();
        let _ = registry.accept("email", "a@example.com".to_string());
        let _ = registry.accept("name", "Alice".to_string());

        registry.reset_field("email");
        assert!(registry.is_empty("email"));
        assert_eq!(registry.len("name"), 1);

        registry.reset();
        assert!(registry.is_empty("name"));
    }

    /// A tiny attempt budget bounds the draws an exhaustion loop can make.
    #[test]
    fn custom_attempt_budget_is_reported() {
        let registry = UniqueRegistry::with_max_attempts(3);
        assert_eq!(registry.max_attempts(), 3);
        assert_eq!(
            UniqueRegistry::default().max_attempts(),
            DEFAULT_MAX_ATTEMPTS
        );
    }
}
