//! `make:test` variants: browser (WebDriver) and feature-scoped.

use crate::common::temp_root;

use rustasea_cli::generators::{generate, Kind, MakeOptions};

/// `make:test --browser` writes a WebDriver e2e test under `tests/browser/`.
#[test]
fn browser_test_generator_writes_webdriver_harness() {
    let root = temp_root();
    let opts = MakeOptions {
        name: "LoginTest".to_string(),
        force: false,
        resource: false,
        with_migration: false,
        browser: true,
    };
    let files = generate(Kind::Test, &root, &opts).expect("browser test scaffold");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "tests/browser/login_test.rs");

    let source = std::fs::read_to_string(root.join("tests/browser/login_test.rs"))
        .expect("read browser test");
    assert!(
        source.contains("use rustasea::testing::browser::{Browser, BrowserError, ServerHandle};"),
        "browser test must import the WebDriver harness: {source}"
    );
    assert!(
        source.contains("ServerHandle::start(app())"),
        "browser test must boot the app on an ephemeral port: {source}"
    );
    assert!(
        source.contains("#[ignore = \"requires WEBDRIVER_URL"),
        "live e2e must be ignored by default: {source}"
    );
    assert!(
        source.contains("BrowserError::NotConfigured"),
        "browser test must cover the skip path: {source}"
    );
    assert!(
        source.contains("browser.screenshot_on_failure(\"login_test-flow\", &result).await;"),
        "browser test must await the screenshot future: {source}"
    );

    // Every async harness call in the emitted source must be awaited: an
    // un-awaited `try_connect()` compiles to a `let ... else` on a future and
    // fails with E0308 (TASK-088).
    assert!(
        source.contains("Browser::try_connect().await"),
        "browser test must await try_connect: {source}"
    );
    assert!(
        !source.contains("Browser::try_connect() else"),
        "browser test must not bind the un-awaited try_connect future: {source}"
    );
    assert!(
        source.contains("ServerHandle::start(app())\n        .await"),
        "browser test must await ServerHandle::start: {source}"
    );
    assert!(
        source.contains("browser.visit(&server.base_url()).await?;"),
        "browser test must await visit: {source}"
    );
    assert!(
        source.contains("browser.assert_see(\"Hello, browser!\").await?;"),
        "browser test must await assert_see: {source}"
    );
    assert!(
        source.contains("browser.close().await"),
        "browser test must await close: {source}"
    );
    assert!(
        source.contains("server.shutdown().await;"),
        "browser test must await shutdown: {source}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `make:test` without `--browser` keeps the `tests/feature/` `TestCase` path.
#[test]
fn default_test_generator_stays_feature_scoped() {
    let root = temp_root();
    let opts = MakeOptions {
        name: "LoginTest".to_string(),
        force: false,
        resource: false,
        with_migration: false,
        browser: false,
    };
    let files = generate(Kind::Test, &root, &opts).expect("feature test scaffold");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "tests/feature/login_test.rs");
    assert!(
        !root.join("tests/browser/login_test.rs").exists(),
        "the default variant must not write a browser test"
    );

    let source = std::fs::read_to_string(root.join("tests/feature/login_test.rs"))
        .expect("read feature test");
    assert!(
        source.contains("TestCase"),
        "feature test must use TestCase"
    );
    let _ = std::fs::remove_dir_all(&root);
}
