/// Password-strength policy mirroring Laravel's `Password` rule object.
///
/// The Laravel livewire-starter-kit configures a global default:
///
/// ```php
/// Password::min(12)->mixedCase()->letters()->numbers()->symbols()->uncompromised()
/// ```
///
/// RustaSea models that as a plain value object passed explicitly on a
/// [`crate::rules::Rules`] instance
/// ([`crate::rules::Rules::password_policy`]) instead of a process-wide
/// singleton, per ADR-0007 (no global mutable state). The policy is pure: it
/// inspects the candidate string in memory and performs no I/O.
///
/// Each violated constraint produces its own error message, so the client can
/// show every unmet requirement at once.
///
/// ```rust
/// use rustasea_validation::{PasswordPolicy, Rules};
/// use rustasea_validation::serde_json::json;
///
/// let rules = Rules::new()
///     .password_policy(PasswordPolicy::new().min(12).mixed_case().numbers())
///     .field("password", "required|string|password");
/// assert!(rules
///     .validate(&json!({ "password": "Sup3rSecretPass" }))
///     .is_ok());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordPolicy {
    /// Minimum number of Unicode scalar values (`None` = no minimum).
    pub min_length: Option<usize>,
    /// Require at least one ASCII letter.
    pub require_letters: bool,
    /// Require both an uppercase and a lowercase ASCII letter.
    pub require_mixed_case: bool,
    /// Require at least one ASCII digit.
    pub require_numbers: bool,
    /// Require at least one non-alphanumeric character.
    pub require_symbols: bool,
    /// Whether the password must be checked against a breach corpus.
    ///
    /// **Off by default and never performed as an HTTP call by this crate.**
    /// Enabling it makes the `password` rule fail closed with a typed
    /// unsupported error, because RustaSea does not ship a HaveIBeenPwned
    /// k-anonymity client. Supply one through a dedicated checker before
    /// relying on this flag; until then the rule refuses to silently pass.
    pub uncompromised: bool,
}

impl Default for PasswordPolicy {
    /// The RustaSea default policy: **minimum 12 characters only**.
    ///
    /// This matches the scaffold's existing `MIN_LENGTH = 12` and keeps the
    /// baseline behaviour of the generated password-update request. It is
    /// intentionally less strict than the kit's production profile: enable
    /// `mixed_case()`, `letters()`, `numbers()` and `symbols()` explicitly
    /// (see [`PasswordPolicy::production`]) when that profile is wanted.
    fn default() -> Self {
        Self {
            min_length: Some(12),
            require_letters: false,
            require_mixed_case: false,
            require_numbers: false,
            require_symbols: false,
            uncompromised: false,
        }
    }
}

/// A single unmet password requirement.
///
/// Carries a stable code and the human message emitted into the
/// [`crate::ErrorBag`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordViolation {
    /// Stable rule code (for example `password_min`).
    pub code: &'static str,
    /// Human-readable message.
    pub message: String,
}

impl PasswordPolicy {
    /// An empty policy (no constraints).
    pub fn new() -> Self {
        Self {
            min_length: None,
            require_letters: false,
            require_mixed_case: false,
            require_numbers: false,
            require_symbols: false,
            uncompromised: false,
        }
    }

    /// The kit's production profile: `min(12)->mixedCase()->letters()
    /// ->numbers()->symbols()`.
    ///
    /// `uncompromised()` is intentionally **not** enabled because this crate
    /// has no breach-list checker; see [`PasswordPolicy::uncompromised`].
    pub fn production() -> Self {
        Self::new()
            .min(12)
            .mixed_case()
            .letters()
            .numbers()
            .symbols()
    }

    /// Set the minimum length in Unicode scalar values.
    pub fn min(mut self, len: usize) -> Self {
        self.min_length = Some(len);
        self
    }

    /// Require at least one ASCII letter.
    pub fn letters(mut self) -> Self {
        self.require_letters = true;
        self
    }

    /// Require both an uppercase and a lowercase ASCII letter.
    pub fn mixed_case(mut self) -> Self {
        self.require_mixed_case = true;
        self
    }

    /// Require at least one ASCII digit.
    pub fn numbers(mut self) -> Self {
        self.require_numbers = true;
        self
    }

    /// Require at least one non-alphanumeric character.
    pub fn symbols(mut self) -> Self {
        self.require_symbols = true;
        self
    }

    /// Require a breach-corpus check.
    ///
    /// **Not implemented as an HTTP call.** Because RustaSea ships no
    /// HaveIBeenPwned checker, enabling this flag makes the `password` rule
    /// fail closed with a typed
    /// [`crate::failure::RuleError::Unsupported`] error rather than silently
    /// passing. This is deliberate: a rule that cannot be evaluated must not
    /// pass.
    pub fn uncompromised(mut self, enabled: bool) -> Self {
        self.uncompromised = enabled;
        self
    }

    /// Evaluate `candidate`, returning every unmet requirement.
    ///
    /// An empty result means the password satisfies the policy.
    pub fn violations(&self, candidate: &str) -> Vec<PasswordViolation> {
        let mut violations = Vec::new();
        if let Some(min) = self.min_length {
            if candidate.chars().count() < min {
                violations.push(PasswordViolation {
                    code: "password_min",
                    message: format!("The password must be at least {min} characters."),
                });
            }
        }
        let has_letter = candidate.chars().any(|c| c.is_ascii_alphabetic());
        if self.require_letters && !has_letter {
            violations.push(PasswordViolation {
                code: "password_letters",
                message: "The password must contain at least one letter.".to_string(),
            });
        }
        if self.require_mixed_case
            && !(candidate.chars().any(|c| c.is_ascii_lowercase())
                && candidate.chars().any(|c| c.is_ascii_uppercase()))
        {
            violations.push(PasswordViolation {
                code: "password_mixed_case",
                message: "The password must contain both uppercase and lowercase letters."
                    .to_string(),
            });
        }
        if self.require_numbers && !candidate.chars().any(|c| c.is_ascii_digit()) {
            violations.push(PasswordViolation {
                code: "password_numbers",
                message: "The password must contain at least one number.".to_string(),
            });
        }
        if self.require_symbols
            && !candidate
                .chars()
                .any(|c| !c.is_alphanumeric() && !c.is_whitespace())
        {
            violations.push(PasswordViolation {
                code: "password_symbols",
                message: "The password must contain at least one symbol.".to_string(),
            });
        }
        violations
    }

    /// Whether `candidate` satisfies every constraint in this policy.
    pub fn is_satisfied_by(&self, candidate: &str) -> bool {
        self.violations(candidate).is_empty() && !self.uncompromised
    }
}

impl Default for PasswordViolation {
    fn default() -> Self {
        Self {
            code: "password",
            message: String::new(),
        }
    }
}
