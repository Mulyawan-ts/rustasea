//! Hermetic unit tests for the browser harness (feature `browser`).
//!
//! These never touch a real browser: they cover selector building, artifact
//! paths, the `NotConfigured` skip classification, and the ephemeral server.
//! Live WebDriver flows live in `tests/browser_e2e.rs` behind `#[ignore]`.

use tokio::sync::Mutex;

use super::browser::{artifact_dir, normalize_selector, sanitize};
use super::{Browser, BrowserError, ServerHandle};
use crate::browser::browser::{ARTIFACT_DIR_ENV, WEBDRIVER_URL_ENV};

/// Serializes tests that mutate process-global environment variables.
static ENV_LOCK: Mutex<()> = Mutex::const_new(());

/// `@name` expands to a `[name="…"]` attribute selector; CSS passes through.
#[test]
fn selector_building_expands_name_shorthand() {
    assert_eq!(normalize_selector("@email"), "[name=\"email\"]");
    assert_eq!(normalize_selector("#login"), "#login");
    assert_eq!(normalize_selector(".btn-primary"), ".btn-primary");
    assert_eq!(
        normalize_selector("button[type=submit]"),
        "button[type=submit]"
    );
}

/// Filenames are sanitized to a safe subset, with an empty-name fallback.
#[test]
fn screenshot_names_are_sanitized() {
    assert_eq!(sanitize("Login flow/1"), "Login_flow_1");
    assert_eq!(sanitize("a.b:c"), "a_b_c");
    assert_eq!(sanitize("ok-name_1"), "ok-name_1");
    assert_eq!(sanitize(""), "screenshot");
}

/// The artifact dir honours `BROWSER_ARTIFACT_DIR`, else defaults.
#[test]
fn artifact_dir_resolution() {
    let _guard = ENV_LOCK.blocking_lock();
    std::env::remove_var(ARTIFACT_DIR_ENV);
    assert_eq!(artifact_dir().to_string_lossy(), "target/browser-artifacts");

    std::env::set_var(ARTIFACT_DIR_ENV, "/tmp/custom-artifacts");
    assert_eq!(artifact_dir().to_string_lossy(), "/tmp/custom-artifacts");
    std::env::remove_var(ARTIFACT_DIR_ENV);
}

/// An unreachable endpoint is classified as the skip signal, not a hard error.
#[tokio::test]
async fn unreachable_endpoint_is_not_configured() {
    let err = match Browser::connect_to("http://127.0.0.1:1").await {
        Ok(_) => panic!("port 1 must be unreachable"),
        Err(err) => err,
    };
    assert!(
        matches!(err, BrowserError::NotConfigured { .. }),
        "expected NotConfigured, got {err:?}"
    );
    assert!(err.is_not_configured());
}

/// `try_connect` returns `None` when `WEBDRIVER_URL` points nowhere.
#[tokio::test]
async fn try_connect_returns_none_when_unconfigured() {
    // Hold the lock across the await so env mutation stays protected for the
    // whole test, not just the `set_var` window.
    let _guard = ENV_LOCK.lock().await;
    std::env::set_var(WEBDRIVER_URL_ENV, "http://127.0.0.1:1");
    let outcome = Browser::try_connect().await;
    std::env::remove_var(WEBDRIVER_URL_ENV);
    assert!(outcome.is_none());
}

/// The ephemeral server binds a real port and serves the supplied router.
#[tokio::test]
async fn server_handle_serves_on_ephemeral_port() {
    use axum::routing::get;
    use axum::Router;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let router = Router::new().route("/", get(|| async { "ok" }));
    let server = ServerHandle::start(router).await.expect("server starts");
    let base_url = server.base_url();
    let port = server.port();

    assert!(port != 0, "ephemeral port must be non-zero");
    assert_eq!(base_url, format!("http://127.0.0.1:{port}"));

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to server");
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .expect("send request");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read response");
    let response = String::from_utf8_lossy(&response);
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "unexpected response: {response}"
    );

    server.shutdown().await;
}
