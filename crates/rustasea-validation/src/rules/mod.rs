/// Validation rule declarations and the `Validatable` trait.
///
/// Rule strings mirror Laravel's validation grammar. The supported rule names
/// are:
///
/// * `required` — the field must be present and non-null.
/// * `string` — the value must be a JSON string.
/// * `email` — the value must be a valid email address.
/// * `timezone` — the value must be a valid IANA timezone name (or a
///   documented short alias such as `PST`/`EST`/`CST`/`MST`/`GMT`).
/// * `min:N` / `max:N` — length bounds (Unicode scalar count for strings,
///   element count for arrays/objects, digit count for numbers).
/// * `in_array:a,b,c` / `in:a,b,c` / `strict_in_array:a,b,c` — strict
///   membership (type + value).
/// * `contains:x` / `contains_strict:x` / `strict_contains:x` — strict
///   containment.
/// * `doesnt_contain:x` — inverse containment.
/// * `confirmed` — the field `X` must equal `X_confirmation` in the same
///   payload (Laravel semantics).
/// * `password` — evaluate the payload value against the
///   [`Rules::password_policy`] (default: minimum 12 characters).
/// * `unique:table,column` — the value must not exist in `table`.`column`.
/// * `unique:table,column,ignore_id` — as above, excluding the row with
///   primary key `ignore_id` (Laravel `Rule::unique()->ignore()`,
///   self-exclusion).
/// * `current_password` — the value must match the authenticated user's
///   current password.
///
/// Parameters use the existing `name:arg` grammar, where `arg` is a
/// comma-separated list (`unique:users,email,42`). Comma splitting is
/// performed by the individual rule, so commas are meaningful only for rules
/// that document them.
///
/// `unique` and `current_password` require external context and therefore run
/// against a [`ValidationContext`]. [`Rules::validate`] keeps its historical
/// signature and passes [`NoContext`], so those rules fail closed with a typed
/// [`RuleError::MissingContext`] unless [`Rules::validate_with`] is used.
use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::context::{NoContext, ValidationContext};
use crate::error_bag::ErrorBag;
use crate::failure::ValidationFailure;
use crate::password::PasswordPolicy;

mod support;
#[cfg(test)]
mod tests;

pub(crate) use support::RuleFailure;

/// Declarative validation rule set for one payload.
#[derive(Debug, Clone, Default)]
pub struct Rules {
    /// field -> ordered rule strings.
    pub fields: HashMap<String, Vec<String>>,
    /// Password policy applied by the `password` rule.
    pub policy: PasswordPolicy,
}

impl Rules {
    /// Create an empty rule set with the default password policy.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add rules for a field (comma/pipe separated, Laravel grammar).
    pub fn add(&mut self, field: impl Into<String>, rules: &str) -> &mut Self {
        let parsed = rules.split('|').map(|r| r.trim()).filter(|r| !r.is_empty());
        self.fields
            .entry(field.into())
            .or_default()
            .extend(parsed.map(String::from));
        self
    }

    /// Fluent builder alias (`Rules::new().field("name", "required|min:3")`).
    pub fn field(mut self, field: impl Into<String>, rules: &str) -> Self {
        self.add(field, rules);
        self
    }

    /// Set the password policy used by the `password` rule.
    ///
    /// Defaults to [`PasswordPolicy::default`] (minimum 12 characters). The
    /// policy is stored on this instance rather than in a process-wide
    /// singleton, per ADR-0007.
    pub fn password_policy(mut self, policy: PasswordPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Validate one JSON payload against the declared rules.
    ///
    /// Keeps the historical `Result<(), ErrorBag>` signature. Context-dependent
    /// rules (`unique`, `current_password`) are evaluated against [`NoContext`]
    /// and therefore fail closed: the returned [`ErrorBag`] carries a typed
    /// `missing_context` entry rather than a silent pass.
    pub fn validate(&self, data: &Value) -> Result<(), ErrorBag> {
        self.run(data, &NoContext)
            .map_err(ValidationFailure::into_error_bag)
    }

    /// Validate with an explicit [`ValidationContext`].
    ///
    /// Returns the same `Result<(), ErrorBag>` shape as [`Rules::validate`],
    /// converting an unevaluable rule into a typed bag entry. Use
    /// [`Rules::validate_checked`] when you need the typed
    /// [`ValidationFailure`].
    pub fn validate_with(&self, data: &Value, ctx: &dyn ValidationContext) -> Result<(), ErrorBag> {
        self.run(data, ctx)
            .map_err(ValidationFailure::into_error_bag)
    }

    /// Validate with an explicit context, preserving the failure kind.
    ///
    /// Unlike [`Rules::validate_with`], this distinguishes
    /// [`ValidationFailure::Invalid`] (the payload is bad) from
    /// [`ValidationFailure::Rule`] (a rule could not be evaluated).
    pub fn validate_checked(
        &self,
        data: &Value,
        ctx: &dyn ValidationContext,
    ) -> Result<(), ValidationFailure> {
        self.run(data, ctx)
    }

    /// Shared evaluation loop used by every public entry point.
    fn run(&self, data: &Value, ctx: &dyn ValidationContext) -> Result<(), ValidationFailure> {
        let mut bag = ErrorBag::new();
        for (field, rules) in &self.fields {
            let value = data.get(field);
            for rule in rules {
                for failure in support::apply_rule(field, value, rule, data, ctx, &self.policy) {
                    match failure {
                        RuleFailure::Invalid(err) => bag.add(field.clone(), err),
                        // Fail closed: an unevaluable rule aborts immediately
                        // with a typed error instead of a generic message.
                        RuleFailure::Context(err) => {
                            return Err(ValidationFailure::Rule(err));
                        }
                    }
                }
            }
        }
        if bag.is_empty() {
            Ok(())
        } else {
            Err(ValidationFailure::Invalid(bag))
        }
    }
}

/// Serialize a payload into a JSON value for rule evaluation.
///
/// Generated form requests call this so they never need a direct
/// `serde_json` dependency of their own.
pub fn to_value<T>(value: T) -> std::result::Result<Value, serde_json::Error>
where
    T: serde::Serialize,
{
    serde_json::to_value(value)
}

/// Payload contract: types that can be validated.
///
/// Mirrors Laravel's `FormRequest` + `validator` derive: parse JSON into `T`
/// (via `DeserializeOwned`), then validate the resulting structure.
pub trait Validatable: DeserializeOwned {
    /// Validate `&self` and return `ErrorBag` on failure.
    fn validate(&self) -> std::result::Result<(), ErrorBag>;

    /// Validate a JSON value by deserializing then validating.
    fn validate_value(value: &Value) -> std::result::Result<Self, ErrorBag> {
        let parsed: Self = serde_json::from_value(value.clone())
            .map_err(|e| ErrorBag::from_message(e.to_string()))?;
        parsed.validate()?;
        Ok(parsed)
    }
}

impl ErrorBag {
    /// Build a bag from a single top-level message.
    pub fn from_message(message: impl Into<String>) -> Self {
        let mut bag = ErrorBag::new();
        bag.add(
            "payload",
            crate::error_bag::ValidationError::new("parse", message),
        );
        bag
    }
}
