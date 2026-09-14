/// Runtime retry/timing policy for queue jobs (`#[tries]`/`#[backoff]`/`#[timeout]`).
///
/// [`JobPolicy`] is the runtime form of the M5 declarative attribute bundle: the
/// macros emit `__RUSTASEA_TRIES_<Type>` / `__RUSTASEA_BACKOFF_SECS_<Type>` /
/// `__RUSTASEA_TIMEOUT_SECS_<Type>` consts, and an application binds them to a
/// job type with [`crate::driver::register_job_with_policy`]. Rust has no
/// reflection, so — mirroring the explicit-registration precedent used for
/// `#[middleware]`/`#[authorize]` routing metadata — the worker reads the
/// policy per job from the registered handler rather than scanning the type.
///
/// When no explicit policy is registered the worker falls back to the job's own
/// [`crate::job::Job::tries`]/`backoff`/`timeout` accessors, so existing jobs are
/// unaffected.
use std::time::Duration;

/// Maximum exponent applied when doubling the backoff (guards the shift).
const MAX_BACKOFF_SHIFT: u32 = 30;

/// Consolidated retry/timing policy attached to a job's execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobPolicy {
    /// Maximum number of attempts (the initial run counts as one).
    pub max_tries: u32,
    /// Base delay before the first retry; later retries double (exponential).
    pub backoff: Duration,
    /// Per-attempt timeout; [`Duration::ZERO`] disables the timeout.
    pub timeout: Duration,
}

impl JobPolicy {
    /// Build a policy from explicit parts.
    pub const fn new(max_tries: u32, backoff: Duration, timeout: Duration) -> Self {
        Self {
            max_tries,
            backoff,
            timeout,
        }
    }

    /// Build a policy from the second-based declarative attribute consts.
    ///
    /// Convenience for wiring `#[tries(n)]`/`#[backoff(secs)]`/`#[timeout(secs)]`
    /// without repeating the unit conversions at the call site.
    pub const fn from_seconds(max_tries: usize, backoff_secs: usize, timeout_secs: usize) -> Self {
        Self {
            max_tries: max_tries as u32,
            backoff: Duration::from_secs(backoff_secs as u64),
            timeout: Duration::from_secs(timeout_secs as u64),
        }
    }

    /// Default policy: a single attempt, no retry, no timeout.
    pub const fn none() -> Self {
        Self {
            max_tries: 1,
            backoff: Duration::ZERO,
            timeout: Duration::ZERO,
        }
    }

    /// Whether another attempt is permitted after `attempts` started attempts.
    pub const fn allows_retry(&self, attempts: u32) -> bool {
        attempts < self.max_tries
    }

    /// Delay before the retry that follows failed attempt `failed_attempt`.
    ///
    /// `failed_attempt` is 1-based (the first failure is `1`). The delay doubles
    /// for every retry beyond the first: attempt 1 waits `backoff`, attempt 2
    /// waits `2 * backoff`, and so on. A zero base backoff yields no delay.
    pub fn delay_for_attempt(&self, failed_attempt: u32) -> Duration {
        if self.backoff.is_zero() || failed_attempt <= 1 {
            return self.backoff;
        }
        let factor = 1u32 << (failed_attempt - 1).min(MAX_BACKOFF_SHIFT);
        self.backoff.saturating_mul(factor)
    }
}

impl Default for JobPolicy {
    /// Default to [`JobPolicy::none`] (one attempt, no delay, no timeout).
    fn default() -> Self {
        Self::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Backoff doubles per retry from the declared base.
    #[test]
    fn backoff_doubles_per_retry() {
        let policy = JobPolicy::new(4, Duration::from_secs(10), Duration::ZERO);
        assert_eq!(policy.delay_for_attempt(1), Duration::from_secs(10));
        assert_eq!(policy.delay_for_attempt(2), Duration::from_secs(20));
        assert_eq!(policy.delay_for_attempt(3), Duration::from_secs(40));
    }

    /// A zero base backoff never delays, and a single try allows no retry.
    #[test]
    fn zero_backoff_and_single_try() {
        let policy = JobPolicy::new(1, Duration::ZERO, Duration::ZERO);
        assert_eq!(policy.delay_for_attempt(1), Duration::ZERO);
        assert_eq!(policy.delay_for_attempt(3), Duration::ZERO);
        assert!(!policy.allows_retry(1));
        assert!(!policy.allows_retry(2));
    }

    /// `from_seconds` mirrors the declarative attribute units.
    #[test]
    fn from_seconds_matches_attribute_units() {
        let policy = JobPolicy::from_seconds(3, 10, 30);
        assert_eq!(policy.max_tries, 3);
        assert_eq!(policy.backoff, Duration::from_secs(10));
        assert_eq!(policy.timeout, Duration::from_secs(30));
    }

    /// The retry budget permits attempts strictly below `max_tries`.
    #[test]
    fn retry_budget_bounds_attempts() {
        let policy = JobPolicy::new(3, Duration::from_secs(1), Duration::ZERO);
        assert!(policy.allows_retry(1));
        assert!(policy.allows_retry(2));
        assert!(!policy.allows_retry(3));
    }
}
