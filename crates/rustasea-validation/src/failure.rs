/// Typed validation outcomes for rules that cannot be evaluated in isolation.
///
/// [`crate::rules::Rules::validate`] keeps its historical
/// `Result<(), ErrorBag>` signature. The context-aware entry points
/// ([`crate::rules::Rules::validate_with`] and
/// [`crate::rules::Rules::validate_checked`]) use this module to distinguish
/// between *the payload is invalid* and *a rule could not be evaluated*.
///
/// Both cases are fail-closed: a rule that cannot be evaluated never silently
/// passes.
use crate::error_bag::{ErrorBag, ValidationError};

/// A rule that could not be evaluated.
///
/// This is deliberately separate from a plain validation failure: callers
/// (for example an HTTP layer) can map it to a 422 "unprocessable" response
/// while logging it differently from an ordinary field error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleError {
    /// A context-dependent rule (`unique`, `current_password`) ran without a
    /// usable [`crate::context::ValidationContext`].
    ///
    /// The rule is fail-closed: it does not pass and it does not emit a
    /// generic validation message.
    MissingContext {
        /// Field the rule was declared on.
        field: String,
        /// Rule name that could not be evaluated (`unique`,
        /// `current_password`).
        rule: String,
    },
    /// A rule was configured in a way this crate cannot evaluate, for example
    /// `unique` without a table/column, or a password policy that enables
    /// `uncompromised` (which needs a breach-list checker this crate does not
    /// provide).
    Unsupported {
        /// Field the rule was declared on.
        field: String,
        /// Rule name that could not be evaluated.
        rule: String,
        /// Human-readable explanation of why the rule is unsupported.
        detail: String,
    },
}

impl RuleError {
    /// The field the failing rule was declared on.
    pub fn field(&self) -> &str {
        match self {
            RuleError::MissingContext { field, .. } | RuleError::Unsupported { field, .. } => field,
        }
    }

    /// The rule name that could not be evaluated.
    pub fn rule(&self) -> &str {
        match self {
            RuleError::MissingContext { rule, .. } | RuleError::Unsupported { rule, .. } => rule,
        }
    }

    /// Stable error code surfaced in the [`ErrorBag`].
    pub fn code(&self) -> &'static str {
        match self {
            RuleError::MissingContext { .. } => "missing_context",
            RuleError::Unsupported { .. } => "unsupported_rule",
        }
    }

    /// Human-readable message surfaced in the [`ErrorBag`].
    pub fn message(&self) -> String {
        let label = self.field().replace(['_', '.'], " ");
        match self {
            RuleError::MissingContext { rule, .. } => format!(
                "The {label} could not be validated: the `{rule}` rule requires a validation \
                 context."
            ),
            RuleError::Unsupported { detail, .. } => {
                format!("The {label} could not be validated: {detail}.")
            }
        }
    }

    /// Convert into a single [`ValidationError`] so callers holding an
    /// [`ErrorBag`] can still surface the typed failure.
    pub fn to_validation_error(&self) -> ValidationError {
        ValidationError::new(self.code(), self.message())
    }
}

impl std::fmt::Display for RuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for RuleError {}

/// Result of evaluating a rule set against a payload.
///
/// Mirrors `Result<(), ErrorBag>` for the common case while preserving the
/// distinction between invalid data and an unevaluable rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationFailure {
    /// One or more fields failed validation.
    Invalid(ErrorBag),
    /// A rule could not be evaluated (fail-closed).
    Rule(RuleError),
}

impl ValidationFailure {
    /// Whether this is an ordinary field-validation failure.
    pub fn is_invalid(&self) -> bool {
        matches!(self, ValidationFailure::Invalid(_))
    }

    /// Whether this is an unevaluable-rule failure.
    pub fn is_rule_error(&self) -> bool {
        matches!(self, ValidationFailure::Rule(_))
    }

    /// The typed rule error, if this is a rule failure.
    pub fn rule_error(&self) -> Option<&RuleError> {
        match self {
            ValidationFailure::Rule(err) => Some(err),
            ValidationFailure::Invalid(_) => None,
        }
    }

    /// Consume the failure into a single [`ErrorBag`], converting a rule error
    /// into a typed entry keyed by its field.
    pub fn into_error_bag(self) -> ErrorBag {
        match self {
            ValidationFailure::Invalid(bag) => bag,
            ValidationFailure::Rule(err) => {
                let mut bag = ErrorBag::new();
                bag.add(err.field().to_string(), err.to_validation_error());
                bag
            }
        }
    }
}

impl From<RuleError> for ValidationFailure {
    fn from(err: RuleError) -> Self {
        ValidationFailure::Rule(err)
    }
}

impl From<ErrorBag> for ValidationFailure {
    fn from(bag: ErrorBag) -> Self {
        ValidationFailure::Invalid(bag)
    }
}

impl std::fmt::Display for ValidationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationFailure::Invalid(bag) => {
                write!(f, "validation failed for {} field(s)", bag.field_count())
            }
            ValidationFailure::Rule(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ValidationFailure {}
