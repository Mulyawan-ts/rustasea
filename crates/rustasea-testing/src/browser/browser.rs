//! Fluent WebDriver browser harness (feature `browser`).
//!
//! [`Browser`] wraps a `fantoccini` WebDriver session with Laravel
//! Dusk / `pestphp/pest-plugin-browser`-style helpers: navigate, fill, click,
//! wait, assert, and screenshot. The endpoint is read from `WEBDRIVER_URL`
//! (default `http://127.0.0.1:4444`); when no driver is reachable the harness
//! returns [`BrowserError::NotConfigured`] so a live test can skip instead of
//! failing the default suite.

use std::path::PathBuf;
use std::time::Duration;

use fantoccini::elements::Element;
use fantoccini::error::CmdError;
use fantoccini::key::Key;
use fantoccini::{Client, ClientBuilder, Locator};

use super::error::{selector_error, session_error, BrowserError, BrowserResult};

/// Environment variable holding the WebDriver endpoint.
pub const WEBDRIVER_URL_ENV: &str = "WEBDRIVER_URL";
/// WebDriver endpoint used when [`WEBDRIVER_URL_ENV`] is unset.
pub const DEFAULT_WEBDRIVER_URL: &str = "http://127.0.0.1:4444";
/// Environment variable overriding the screenshot artifact directory.
pub const ARTIFACT_DIR_ENV: &str = "BROWSER_ARTIFACT_DIR";
/// Default screenshot artifact directory (relative to the working directory).
pub const DEFAULT_ARTIFACT_DIR: &str = "target/browser-artifacts";

/// A live WebDriver browser session with fluent test helpers.
pub struct Browser {
    client: Client,
}

impl Browser {
    /// Connect to the WebDriver endpoint from `WEBDRIVER_URL`.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::NotConfigured`] when the endpoint is unreachable
    /// (the skip signal), or [`BrowserError::WebDriver`] for any other failure.
    pub async fn connect() -> BrowserResult<Self> {
        let url =
            std::env::var(WEBDRIVER_URL_ENV).unwrap_or_else(|_| DEFAULT_WEBDRIVER_URL.to_string());
        Self::connect_to(&url).await
    }

    /// Connect to an explicit WebDriver endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::NotConfigured`] when the endpoint is unreachable
    /// (the skip signal), or [`BrowserError::WebDriver`] for any other failure.
    pub async fn connect_to(url: &str) -> BrowserResult<Self> {
        ensure_crypto_provider();
        let builder = ClientBuilder::rustls()
            .map_err(|err| BrowserError::WebDriver(format!("rustls init failed: {err}")))?;
        let client = builder
            .connect(url)
            .await
            .map_err(|err| session_error(url, err))?;
        Ok(Self { client })
    }

    /// Connect if a WebDriver endpoint is available, else `None`.
    ///
    /// A convenience for live tests that want to skip silently when no browser
    /// is configured; unexpected failures are reported to stderr.
    pub async fn try_connect() -> Option<Self> {
        match Self::connect().await {
            Ok(browser) => Some(browser),
            Err(err) => {
                if !err.is_not_configured() {
                    eprintln!("browser harness unavailable: {err}");
                }
                None
            }
        }
    }

    /// Navigate to `url` (Dusk `$browser->visit()`).
    ///
    /// # Errors
    ///
    /// Returns a WebDriver error when navigation fails.
    pub async fn visit(&self, url: &str) -> BrowserResult<()> {
        self.client.goto(url).await?;
        Ok(())
    }

    /// Clear `selector` and type `value` (Dusk `$browser->type()`).
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::Selector`] when `selector` matches nothing.
    pub async fn fill(&self, selector: &str, value: &str) -> BrowserResult<()> {
        let field = self.find(selector).await?;
        field.clear().await?;
        field.send_keys(value).await?;
        Ok(())
    }

    /// Click the element at `selector` (Dusk `$browser->click()`).
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::Selector`] when `selector` matches nothing.
    pub async fn click(&self, selector: &str) -> BrowserResult<()> {
        self.find(selector).await?.click().await?;
        Ok(())
    }

    /// Select the option with `value` in a `<select>` (Dusk `$browser->select()`).
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::Selector`] when `selector` matches nothing.
    pub async fn select(&self, selector: &str, value: &str) -> BrowserResult<()> {
        self.find(selector).await?.select_by_value(value).await?;
        Ok(())
    }

    /// Check a checkbox/radio if unchecked (Dusk `$browser->check()`).
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::Selector`] when `selector` matches nothing.
    pub async fn check(&self, selector: &str) -> BrowserResult<()> {
        let field = self.find(selector).await?;
        if !field.is_selected().await? {
            field.click().await?;
        }
        Ok(())
    }

    /// Uncheck a checkbox if checked (Dusk `$browser->uncheck()`).
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::Selector`] when `selector` matches nothing.
    pub async fn uncheck(&self, selector: &str) -> BrowserResult<()> {
        let field = self.find(selector).await?;
        if field.is_selected().await? {
            field.click().await?;
        }
        Ok(())
    }

    /// Send a key to the focused element (Dusk `$browser->keys()`).
    ///
    /// Named keys (`"Enter"`, `"Tab"`, `"Escape"`, …) are translated to their
    /// WebDriver key code; anything else is typed literally.
    ///
    /// # Errors
    ///
    /// Returns a WebDriver error when no element is focused or the key fails.
    pub async fn press(&self, key: &str) -> BrowserResult<()> {
        let element = self.client.active_element().await?;
        element.send_keys(&key_code(key)).await?;
        Ok(())
    }

    /// Wait up to `timeout` for `selector` to appear (Dusk `$browser->waitFor()`).
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::Timeout`] when the deadline passes.
    pub async fn wait_for(&self, selector: &str, timeout: Duration) -> BrowserResult<()> {
        let css = normalize_selector(selector);
        match self
            .client
            .wait()
            .at_most(timeout)
            .for_element(Locator::Css(&css))
            .await
        {
            Ok(_) => Ok(()),
            Err(CmdError::WaitTimeout) => Err(BrowserError::Timeout {
                selector: css,
                secs: timeout.as_secs(),
            }),
            Err(other) => Err(selector_error(&css, other)),
        }
    }

    /// Assert `selector`'s text contains `expected` (Dusk `assertSeeIn()`).
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::Assertion`] when the text does not contain it.
    pub async fn assert_text(&self, selector: &str, expected: &str) -> BrowserResult<()> {
        let actual = self.find(selector).await?.text().await?;
        if actual.contains(expected) {
            Ok(())
        } else {
            Err(BrowserError::Assertion(format!(
                "expected `{selector}` text to contain {expected:?}, found {actual:?}"
            )))
        }
    }

    /// Assert the rendered page text contains `text` (Dusk `assertSee()`).
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::Assertion`] when the text is absent.
    pub async fn assert_see(&self, text: &str) -> BrowserResult<()> {
        let body = self.body_text().await?;
        if body.contains(text) {
            Ok(())
        } else {
            Err(BrowserError::Assertion(format!(
                "expected page to contain {text:?}"
            )))
        }
    }

    /// Assert the rendered page text does not contain `text` (Dusk `assertDontSee()`).
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::Assertion`] when the text is present.
    pub async fn assert_dont_see(&self, text: &str) -> BrowserResult<()> {
        let body = self.body_text().await?;
        if body.contains(text) {
            Err(BrowserError::Assertion(format!(
                "expected page not to contain {text:?}"
            )))
        } else {
            Ok(())
        }
    }

    /// Assert the current URL equals `expected` or ends with it (Dusk `assertUrlIs()`).
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::Assertion`] when neither holds.
    pub async fn assert_url(&self, expected: &str) -> BrowserResult<()> {
        let url = self.client.current_url().await?;
        let current = url.as_str();
        if current == expected || current.ends_with(expected) {
            Ok(())
        } else {
            Err(BrowserError::Assertion(format!(
                "expected URL {expected:?}, found {current:?}"
            )))
        }
    }

    /// Assert the current URL path equals `path` (Dusk `assertPathIs()`).
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::Assertion`] when the path differs.
    pub async fn assert_path(&self, path: &str) -> BrowserResult<()> {
        let url = self.client.current_url().await?;
        if url.path() == path {
            Ok(())
        } else {
            Err(BrowserError::Assertion(format!(
                "expected path {path:?}, found {:?}",
                url.path()
            )))
        }
    }

    /// Capture a PNG screenshot into the artifact directory.
    ///
    /// The directory comes from [`ARTIFACT_DIR_ENV`] (default
    /// [`DEFAULT_ARTIFACT_DIR`]) and is created on demand; the file is named
    /// `{name}-{timestamp}.png` with non-alphanumeric characters sanitized.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::Screenshot`] when capture or the write fails.
    pub async fn screenshot(&self, name: &str) -> BrowserResult<PathBuf> {
        let bytes = self.client.screenshot().await?;
        let dir = artifact_dir();
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|err| BrowserError::Screenshot(format!("create {}: {err}", dir.display())))?;
        let file = dir.join(format!("{}-{}.png", sanitize(name), timestamp()));
        tokio::fs::write(&file, &bytes)
            .await
            .map_err(|err| BrowserError::Screenshot(format!("write {}: {err}", file.display())))?;
        Ok(file)
    }

    /// On failure, screenshot and report the artifact path (Dusk `->fit()` capture).
    ///
    /// Does nothing when `result` is `Ok`; on `Err` writes a screenshot and
    /// prints the path (or the screenshot failure) to stderr.
    pub async fn screenshot_on_failure(&self, name: &str, result: &Result<(), BrowserError>) {
        let Err(err) = result else {
            return;
        };
        match self.screenshot(name).await {
            Ok(path) => {
                eprintln!(
                    "browser test `{name}` failed: {err}; screenshot: {}",
                    path.display()
                )
            }
            Err(shot_err) => {
                eprintln!("browser test `{name}` failed: {err}; screenshot failed: {shot_err}")
            }
        }
    }

    /// Close the WebDriver session (Dusk `$browser->quit()`).
    ///
    /// # Errors
    ///
    /// Returns a WebDriver error when the session cannot be closed cleanly.
    pub async fn close(self) -> BrowserResult<()> {
        self.client.close().await?;
        Ok(())
    }

    /// Find one element, classifying a miss as [`BrowserError::Selector`].
    async fn find(&self, selector: &str) -> BrowserResult<Element> {
        let css = normalize_selector(selector);
        self.client
            .find(Locator::Css(&css))
            .await
            .map_err(|err| selector_error(&css, err))
    }

    /// The rendered `document.body.innerText` of the current page.
    async fn body_text(&self) -> BrowserResult<String> {
        let value = self
            .client
            .execute(
                "return document.body ? document.body.innerText : '';",
                Vec::new(),
            )
            .await?;
        Ok(value.as_str().unwrap_or_default().to_string())
    }
}

/// Install a process-level rustls crypto provider if none is set.
///
/// `fantoccini` (via hyper-rustls) and `testcontainers` (via bollard) enable
/// different rustls providers, so rustls 0.23 cannot auto-select one and panics
/// on connector build. `ring` is already compiled in, so it is chosen here; a
/// caller that installed its own provider first keeps it.
fn ensure_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Resolve the screenshot artifact directory from the environment.
pub(crate) fn artifact_dir() -> PathBuf {
    std::env::var(ARTIFACT_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_ARTIFACT_DIR))
}

/// Normalize a selector: `@name` expands to `[name="name"]`, else verbatim.
pub(crate) fn normalize_selector(selector: &str) -> String {
    match selector.strip_prefix('@') {
        Some(name) => format!("[name=\"{name}\"]"),
        None => selector.to_string(),
    }
}

/// Replace characters that are unsafe in a filename with `_`.
pub(crate) fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "screenshot".to_string()
    } else {
        cleaned
    }
}

/// A sortable UTC timestamp with millisecond precision.
fn timestamp() -> String {
    chrono::Utc::now().format("%Y%m%d-%H%M%S-%3f").to_string()
}

/// Translate a named key to its WebDriver key code, else return it verbatim.
fn key_code(key: &str) -> String {
    match key.to_ascii_lowercase().as_str() {
        "enter" | "return" => Key::Enter.to_string(),
        "tab" => Key::Tab.to_string(),
        "escape" | "esc" => Key::Escape.to_string(),
        "space" => Key::Space.to_string(),
        "backspace" => Key::Backspace.to_string(),
        "delete" => Key::Delete.to_string(),
        "up" | "arrowup" => Key::Up.to_string(),
        "down" | "arrowdown" => Key::Down.to_string(),
        "left" | "arrowleft" => Key::Left.to_string(),
        "right" | "arrowright" => Key::Right.to_string(),
        "home" => Key::Home.to_string(),
        "end" => Key::End.to_string(),
        _ => key.to_string(),
    }
}
