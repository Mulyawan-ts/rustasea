/// Validation crate for rustasea-validation.
///
/// RustaSea validation layers strict, type+value rules (`in_array`,
/// `contains`, `doesnt_contain`) over the `validator` crate and groups
/// failures into an `ErrorBag` keyed by field. The `FormRequest` trait
/// gives handlers a Laravel-style `validated()` payload.
///
/// Auth-oriented rules (`confirmed`, `string`, `password`, `unique`,
/// `current_password`) extend the rule grammar. Rules that need external
/// information (`unique`, `current_password`) take an explicit
/// [`ValidationContext`] through [`Rules::validate_with`]; they fail closed
/// when no context is available, and [`Rules::validate`] keeps its historical
/// signature.
pub mod context;
pub mod error_bag;
pub mod failure;
pub mod form_request;
pub mod password;
pub mod rules;
pub mod strict;

pub use context::{MemoryContext, NoContext, ValidationContext};
pub use error_bag::{ErrorBag, ValidationError};
pub use failure::{RuleError, ValidationFailure};
pub use form_request::FormRequest;
pub use password::{PasswordPolicy, PasswordViolation};
pub use rules::{to_value, Rules, Validatable};
pub use strict::{contains_strict, doesnt_contain, in_array_strict, matches_strict, StrictValue};

/// Re-exported JSON crate so downstream apps can build payload values without
/// declaring `serde_json` themselves.
pub use serde_json;
