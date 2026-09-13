//! Integration tests for the AUTH-005 auth validation rules.
//!
//! Covers the Laravel livewire-starter-kit contracts ported by AUTH-005:
//! `passwordRules()`, `currentPasswordRules()`, `nameRules()` and
//! `emailRules($userId)` — plus the `string`/`confirmed` type and equality
//! rules and the explicit [`ValidationContext`] seam that keeps `unique` and
//! `current_password` out of the ORM/auth crates.

use rustasea_validation::serde_json::json;
use rustasea_validation::{MemoryContext, NoContext, Rules, ValidationContext};

/// `nameRules()` = `['required','string','max:255']`.
#[test]
fn name_rules_contract() {
    let rules = Rules::new().field("name", "required|string|max:255");
    assert!(rules.validate(&json!({ "name": "Ada Lovelace" })).is_ok());
    assert!(rules.validate(&json!({ "name": 42 })).is_err());
    assert!(rules.validate(&json!({})).is_err());
}

/// `emailRules($userId)` = `['required','string','email','max:255', unique]`.
///
/// The self-exclusion case re-submits the user's own unchanged email.
#[test]
fn email_rules_contract_with_self_exclusion() {
    let rules = Rules::new().field(
        "email",
        "required|string|email|max:255|unique:users,email,7",
    );
    let ctx = MemoryContext::new().with_unique_owned("users", "email", "ada@example.com", "7");

    // Re-submitting own unchanged email passes (self-exclusion).
    assert!(rules
        .validate_with(&json!({ "email": "ada@example.com" }), &ctx)
        .is_ok());

    // A new email passes too.
    assert!(rules
        .validate_with(&json!({ "email": "grace@example.com" }), &ctx)
        .is_ok());

    // A different user's email conflicts.
    let taken = MemoryContext::new().with_unique("users", "email", "taken@example.com");
    let bag = rules
        .validate_with(&json!({ "email": "taken@example.com" }), &taken)
        .unwrap_err();
    assert!(bag.get("email").iter().any(|e| e.code == "unique"));
}

/// `passwordRules()` = `['required','string', Password::default(), 'confirmed']`.
///
/// The default policy is min 12; `confirmed` compares against
/// `password_confirmation`.
#[test]
fn password_rules_contract() {
    let rules = Rules::new().field("password", "required|string|password|confirmed");

    // Matching confirmation and a policy-satisfying value.
    assert!(rules
        .validate(&json!({
            "password": "twelvechars!",
            "password_confirmation": "twelvechars!"
        }))
        .is_ok());

    // Mismatched confirmation.
    let bag = rules
        .validate(&json!({
            "password": "twelvechars!",
            "password_confirmation": "different12!"
        }))
        .unwrap_err();
    assert!(bag.get("password").iter().any(|e| e.code == "confirmed"));

    // Too short.
    let bag = rules
        .validate(&json!({ "password": "short", "password_confirmation": "short" }))
        .unwrap_err();
    assert!(bag.get("password").iter().any(|e| e.code == "password_min"));
}

/// A production-grade policy enforces mixed case, letters, numbers and symbols.
#[test]
fn production_password_policy_contract() {
    use rustasea_validation::PasswordPolicy;
    let rules = Rules::new()
        .password_policy(PasswordPolicy::production())
        .field("password", "required|string|password");
    assert!(rules
        .validate(&json!({ "password": "Sup3rSecret!" }))
        .is_ok());
    let bag = rules
        .validate(&json!({ "password": "alllowercase1" }))
        .unwrap_err();
    assert!(bag
        .get("password")
        .iter()
        .any(|e| e.code == "password_mixed_case"));
    assert!(bag
        .get("password")
        .iter()
        .any(|e| e.code == "password_symbols"));
}

/// `currentPasswordRules()` = `['required','string','current_password']`.
#[test]
fn current_password_rules_contract() {
    let rules = Rules::new().field("current_password", "required|string|current_password");
    let ctx = MemoryContext::new().with_current_password("old-secret");

    assert!(rules
        .validate_with(&json!({ "current_password": "old-secret" }), &ctx)
        .is_ok());

    let bag = rules
        .validate_with(&json!({ "current_password": "wrong" }), &ctx)
        .unwrap_err();
    assert!(bag
        .get("current_password")
        .iter()
        .any(|e| e.code == "current_password"));
}

/// `NoContext` makes context-dependent rules fail closed with a typed error.
#[test]
fn no_context_fails_closed() {
    let unique = Rules::new().field("email", "required|unique:users,email");
    let failure = unique
        .validate_checked(&json!({ "email": "a@example.com" }), &NoContext)
        .unwrap_err();
    assert_eq!(
        failure.rule_error().map(|e| e.code()),
        Some("missing_context")
    );

    let current = Rules::new().field("current_password", "required|current_password");
    let failure = current
        .validate_checked(&json!({ "current_password": "x" }), &NoContext)
        .unwrap_err();
    assert_eq!(
        failure.rule_error().map(|e| e.code()),
        Some("missing_context")
    );

    // The historical `validate` still surfaces a typed bag entry.
    let bag = unique
        .validate(&json!({ "email": "a@example.com" }))
        .unwrap_err();
    assert!(bag.get("email").iter().any(|e| e.code == "missing_context"));
}

/// A caller can implement `ValidationContext` without depending on the ORM or
/// auth crates — the whole point of the seam.
#[test]
fn custom_context_implementation_compiles() {
    struct InMemory {
        taken: Vec<(String, String, String)>,
        current: Option<String>,
    }

    impl ValidationContext for InMemory {
        fn is_unique(
            &self,
            table: &str,
            column: &str,
            value: &str,
            _ignore_id: Option<&str>,
        ) -> Option<bool> {
            Some(
                !self
                    .taken
                    .iter()
                    .any(|(t, c, v)| t == table && c == column && v == value),
            )
        }

        fn current_password_matches(&self, plaintext: &str) -> Option<bool> {
            self.current.as_deref().map(|c| c == plaintext)
        }
    }

    let ctx = InMemory {
        taken: vec![("users".into(), "email".into(), "ada@example.com".into())],
        current: Some("secret".into()),
    };
    let rules = Rules::new()
        .field("email", "required|unique:users,email")
        .field("current_password", "required|current_password");

    assert!(rules
        .validate_with(
            &json!({ "email": "new@example.com", "current_password": "secret" }),
            &ctx
        )
        .is_ok());

    let bag = rules
        .validate_with(
            &json!({ "email": "ada@example.com", "current_password": "secret" }),
            &ctx,
        )
        .unwrap_err();
    assert!(bag.get("email").iter().any(|e| e.code == "unique"));
}
