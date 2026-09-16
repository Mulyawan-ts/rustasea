//! `make:test` template — `tests/feature/<snake>.rs` (TestCase harness) or,
//! with `--browser`, `tests/browser/<snake>.rs` (WebDriver e2e harness).
//!
//! Path and filename follow the sprint-06 M5 acceptance: `make:test UserTest`
//! writes `tests/feature/user_test.rs` (snake_case of the class name), matching
//! the README `tests/feature/` layout. `make:test --browser UserTest` instead
//! writes `tests/browser/user_test.rs` for the feature-gated browser harness.

use std::path::Path;

use crate::error::CliResult;
use crate::generator::Generated;
use crate::generators::kinds::{slug, write_scaffold};
use crate::generators::MakeOptions;

/// Render and write the feature or browser test file.
pub fn scaffold(root: &Path, opts: &MakeOptions) -> CliResult<Generated> {
    if opts.browser {
        scaffold_browser(root, opts)
    } else {
        scaffold_feature(root, opts)
    }
}

/// Render the default `TestCase` feature test (`tests/feature/`).
fn scaffold_feature(root: &Path, opts: &MakeOptions) -> CliResult<Generated> {
    let rel = format!("tests/feature/{}.rs", slug(&opts.name));
    let source = format!(
        r#"//! Feature test scaffold — {name}.
//!
//! Extend `TestCase` and override `setup` to provision isolated stores via
//! the M5 harness; factory sequences reset between tests.

use rustasea::testing::TestCase;

/// Feature test exercising the {kind} flow.
pub struct {name};

impl Default for {name} {{
    /// Build a test with default configuration.
    fn default() -> Self {{
        Self
    }}
}}

impl TestCase for {name} {{
    /// Provision isolated test stores.
    fn setup(&mut self) {{}}
}}

#[cfg(test)]
mod tests {{
    use super::*;

    /// Smoke: setup completes and the harness resets factories.
    #[test]
    fn smoke() {{
        let mut test = {name}::default();
        test.harness_setup();
        rustasea::testing::reset_factory_sequences();
    }}
}}
"#,
        kind = slug(&opts.name),
        name = opts.name,
    );
    write_scaffold(root, rel, source, opts.force)
}

/// Render the WebDriver browser e2e test (`tests/browser/`).
fn scaffold_browser(root: &Path, opts: &MakeOptions) -> CliResult<Generated> {
    let rel = format!("tests/browser/{}.rs", slug(&opts.name));
    let source = format!(
        r#"//! Browser e2e test scaffold — {name}.
//!
//! Boots an axum app on an ephemeral port and drives it through a real
//! WebDriver session (chromedriver/geckodriver), mirroring Laravel Dusk and
//! `pestphp/pest-plugin-browser`: fluent `visit`/`fill`/`click`/`assert_see`
//! helpers over a live browser.
//!
//! Enable the harness in this app's `Cargo.toml`:
//! `rustasea = {{ features = ["browser", ...] }}`.
//!
//! The e2e test is `#[ignore]`d so the default `cargo test` run never needs a
//! browser. Run it explicitly with `WEBDRIVER_URL` pointing at a running
//! chromedriver/geckodriver:
//!
//! ```sh
//! WEBDRIVER_URL=http://127.0.0.1:9515 cargo test --test {slug} -- --ignored
//! ```

use axum::routing::get;
use axum::{{response::Html, Router}};
use rustasea::testing::browser::{{Browser, BrowserError, ServerHandle}};

/// Build the app under test.
///
/// Replace this with the application's real router (e.g. `routes::router`).
fn app() -> Router {{
    Router::new().route("/", get(|| async {{ Html("<h1>Hello, browser!</h1>") }}))
}}

/// End-to-end browser flow — requires a live WebDriver.
#[tokio::test(flavor = "current_thread")]
#[ignore = "requires WEBDRIVER_URL and a matching chromedriver/geckodriver"]
async fn {slug}_browser_flow() {{
    let Some(browser) = Browser::try_connect().await else {{
        eprintln!("skipping {name}: set WEBDRIVER_URL and start chromedriver/geckodriver");
        return;
    }};
    let server = ServerHandle::start(app())
        .await
        .expect("ephemeral test server binds");

    let result: Result<(), BrowserError> = async {{
        browser.visit(&server.base_url()).await?;
        browser.assert_see("Hello, browser!").await?;
        Ok(())
    }}
    .await;

    browser.screenshot_on_failure("{slug}-flow", &result).await;

    browser.close().await.expect("browser session closes");
    server.shutdown().await;

    result.expect("browser flow succeeds");
}}

/// Without a WebDriver endpoint the harness reports a skip signal, not a failure.
#[test]
fn unreachable_webdriver_is_not_configured() {{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime builds");
    let outcome = runtime.block_on(Browser::connect_to("http://127.0.0.1:1"));
    assert!(
        matches!(outcome, Err(BrowserError::NotConfigured {{ .. }})),
        "an unreachable WebDriver must classify as NotConfigured so tests can skip"
    );
}}
"#,
        name = opts.name,
        slug = slug(&opts.name),
    );
    write_scaffold(root, rel, source, opts.force)
}
