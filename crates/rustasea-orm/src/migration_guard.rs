//! Production destructive-command guard (kit `DB::prohibitDestructiveCommands`).
//!
//! Laravel's `AppServiceProvider::configureDefaults()` calls
//! `DB::prohibitDestructiveCommands($this->app->isProduction())`, which refuses
//! `migrate:fresh`, `db:wipe` and friends in production unless `--force` is
//! passed. RustaSea models the decision as a pure function so the CLI (which
//! resolves the environment) and any other caller share a single policy.
//!
//! The guard is deliberately *fail-closed*: an unset `APP_ENV` resolves to
//! [`PRODUCTION`] (Laravel's default), so a destructive command is refused until
//! the caller proves otherwise with `--force`.

/// Environment name treated as production (Laravel's default `APP_ENV`).
pub const PRODUCTION: &str = "production";

/// Error raised when a destructive command is refused in a protected environment.
///
/// Carries the offending command and the environment that triggered the refusal
/// so the CLI can surface a typed, actionable message naming both.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "refusing to run `{command}` in the `{environment}` environment; pass `--force` to override"
)]
pub struct DestructiveCommandRefused {
    /// The destructive command that was refused.
    pub command: String,
    /// The environment that triggered the refusal.
    pub environment: String,
}

/// Whether `environment` names production (case-insensitive, trimmed).
///
/// A blank or unset environment is **not** treated as production here; callers
/// that default an unset environment must default it to [`PRODUCTION`] before
/// calling, which the CLI does.
pub fn is_production(environment: &str) -> bool {
    environment.trim().eq_ignore_ascii_case(PRODUCTION)
}

/// Guard a destructive command against running in production.
///
/// Returns [`DestructiveCommandRefused`] when `environment` is production and
/// `force` is `false`; otherwise `Ok(())`. Pure: no I/O and no global state.
pub fn guard_destructive_command(
    command: &str,
    environment: &str,
    force: bool,
) -> Result<(), DestructiveCommandRefused> {
    if force || !is_production(environment) {
        return Ok(());
    }
    Err(DestructiveCommandRefused {
        command: command.to_string(),
        environment: environment.trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Production is matched case-insensitively and ignores surrounding space.
    #[test]
    fn is_production_is_case_insensitive() {
        assert!(is_production("production"));
        assert!(is_production("PRODUCTION"));
        assert!(is_production("  Production  "));
        assert!(!is_production("local"));
        assert!(!is_production("staging"));
        assert!(!is_production(""));
    }

    /// A non-production environment runs without `--force`.
    #[test]
    fn non_production_is_allowed() {
        assert!(guard_destructive_command("migrate:fresh", "local", false).is_ok());
        assert!(guard_destructive_command("migrate:fresh", "staging", false).is_ok());
    }

    /// Production with `--force` is allowed.
    #[test]
    fn production_with_force_is_allowed() {
        assert!(guard_destructive_command("migrate:fresh", "production", true).is_ok());
    }

    /// Production without `--force` is refused with a typed error naming both.
    #[test]
    fn production_without_force_is_refused() {
        let error = guard_destructive_command("migrate:fresh", "production", false)
            .expect_err("production must refuse");
        assert_eq!(error.command, "migrate:fresh");
        assert_eq!(error.environment, "production");
        let message = error.to_string();
        assert!(message.contains("production"), "message: {message}");
        assert!(message.contains("--force"), "message: {message}");
    }
}
