//! Browser harness error surface (feature `browser`).
//!
//! [`BrowserError::NotConfigured`] is the skip signal: it is returned whenever
//! the WebDriver endpoint is missing or unreachable, so an ignored/live e2e
//! test can degrade to a skip instead of failing the default `cargo test` run.

use fantoccini::error::{CmdError, ErrorStatus, NewSessionError};

/// Errors raised by the browser harness.
#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    /// No reachable WebDriver endpoint — the caller should skip the test.
    #[error(
        "no WebDriver endpoint at {url}; set WEBDRIVER_URL and start chromedriver/geckodriver to run browser tests"
    )]
    NotConfigured {
        /// The endpoint that could not be reached.
        url: String,
    },

    /// The WebDriver session failed for a reason other than reachability.
    #[error("webdriver error: {0}")]
    WebDriver(String),

    /// A selector matched no element.
    #[error("selector matched no element: {0}")]
    Selector(String),

    /// A wait operation exceeded its deadline.
    #[error("timed out after {secs}s waiting for `{selector}`")]
    Timeout {
        /// The selector that never resolved.
        selector: String,
        /// The deadline, in seconds.
        secs: u64,
    },

    /// An assertion did not hold.
    #[error("assertion failed: {0}")]
    Assertion(String),

    /// The ephemeral app server could not start.
    #[error("test server error: {0}")]
    Server(String),

    /// A screenshot could not be captured or written.
    #[error("screenshot error: {0}")]
    Screenshot(String),
}

impl BrowserError {
    /// Whether this error means "no browser environment configured".
    ///
    /// Callers use this to skip a live test rather than fail it.
    pub fn is_not_configured(&self) -> bool {
        matches!(self, BrowserError::NotConfigured { .. })
    }
}

/// Classify a session-creation failure against the endpoint that was tried.
///
/// Connection-level failures (`Failed`/`FailedC`/`Lost`) and a malformed
/// endpoint map to [`BrowserError::NotConfigured`] so the test skips;
/// everything else is a real WebDriver error.
pub(crate) fn session_error(url: &str, err: NewSessionError) -> BrowserError {
    match err {
        NewSessionError::Failed(_)
        | NewSessionError::FailedC(_)
        | NewSessionError::Lost(_)
        | NewSessionError::BadWebdriverUrl(_) => BrowserError::NotConfigured {
            url: url.to_string(),
        },
        other => BrowserError::WebDriver(other.to_string()),
    }
}

impl From<CmdError> for BrowserError {
    /// Lift a WebDriver command failure into the browser error surface.
    fn from(err: CmdError) -> Self {
        BrowserError::WebDriver(err.to_string())
    }
}

/// Classify an element lookup failure: a missing element is a
/// [`BrowserError::Selector`], anything else stays a WebDriver error.
pub(crate) fn selector_error(selector: &str, err: CmdError) -> BrowserError {
    match err {
        CmdError::Standard(ref w) if w.error == ErrorStatus::NoSuchElement => {
            BrowserError::Selector(selector.to_string())
        }
        other => BrowserError::WebDriver(other.to_string()),
    }
}

/// Result alias for the browser harness.
pub type BrowserResult<T> = Result<T, BrowserError>;
