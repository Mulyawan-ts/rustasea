//! Browser testing harness (feature `browser`).
//!
//! Drives a real WebDriver session (chromedriver/geckodriver) through
//! `fantoccini` with Laravel Dusk / `pestphp/pest-plugin-browser`-style fluent
//! helpers, plus [`ServerHandle`] to boot the app under test on an ephemeral
//! port.
//!
//! Enable with the `browser` cargo feature. Live tests should be gated with
//! `#[ignore = "requires WEBDRIVER_URL ..."]`; when no driver is reachable the
//! harness returns [`BrowserError::NotConfigured`] (or `None` from
//! [`Browser::try_connect`]) so the default `cargo test` run never fails for a
//! missing browser.

// `browser/browser.rs` mirrors the module path the ADOPT-029 spec names; the
// inception lint is purely cosmetic here.
#[allow(clippy::module_inception)]
mod browser;
mod error;
mod server;

pub use browser::{
    Browser, ARTIFACT_DIR_ENV, DEFAULT_ARTIFACT_DIR, DEFAULT_WEBDRIVER_URL, WEBDRIVER_URL_ENV,
};
pub use error::{BrowserError, BrowserResult};
pub use server::ServerHandle;

#[cfg(test)]
mod tests;
