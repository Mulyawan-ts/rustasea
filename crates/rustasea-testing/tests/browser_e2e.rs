//! Live browser e2e example (feature `browser`).
//!
//! Every test is `#[ignore]`-gated: it needs a reachable WebDriver endpoint
//! (`WEBDRIVER_URL`) plus a matching `chromedriver`/`geckodriver` and browser.
//! Run with:
//!
//! ```text
//! WEBDRIVER_URL=http://127.0.0.1:4444 \
//!   cargo test -p rustasea-testing --features browser --test browser_e2e -- --ignored
//! ```
//!
//! The tests build a minimal axum app in-process (registration, login, session
//! dashboard, logout) and drive it through the fluent [`Browser`] helpers, so
//! they double as a runnable example of the harness API.

#![cfg(feature = "browser")]

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Form, Router};
use rustasea_testing::browser::{Browser, ServerHandle};
use serde::Deserialize;

/// Shared in-memory app state: registered users + live sessions.
#[derive(Clone, Default)]
struct AppState {
    users: Arc<Mutex<HashMap<String, String>>>,
    sessions: Arc<Mutex<HashSet<String>>>,
    next_token: Arc<Mutex<u64>>,
}

/// A login/registration payload.
#[derive(Deserialize)]
struct Credentials {
    email: String,
    password: String,
}

impl AppState {
    /// Seed the app with one known account.
    fn seeded() -> Self {
        let state = Self::default();
        state
            .users
            .lock()
            .expect("users lock")
            .insert("ada@example.com".to_string(), "password123".to_string());
        state
    }

    /// Register a new account.
    fn register(&self, email: &str, password: &str) {
        self.users
            .lock()
            .expect("users lock")
            .insert(email.to_string(), password.to_string());
    }

    /// Create a session token for `email` when the password matches.
    fn login(&self, email: &str, password: &str) -> Option<String> {
        let ok = self
            .users
            .lock()
            .expect("users lock")
            .get(email)
            .is_some_and(|stored| stored == password);
        if !ok {
            return None;
        }
        let mut next = self.next_token.lock().expect("token lock");
        *next += 1;
        let token = format!("s{next}");
        self.sessions
            .lock()
            .expect("sessions lock")
            .insert(token.clone());
        Some(token)
    }

    /// Whether `headers` carry a live session cookie.
    fn authenticated(&self, headers: &HeaderMap) -> bool {
        let Some(raw) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) else {
            return false;
        };
        raw.split(';').any(|pair| {
            pair.trim()
                .strip_prefix("session=")
                .is_some_and(|token| self.sessions.lock().expect("sessions lock").contains(token))
        })
    }
}

/// Render the login page, optionally with an error banner.
fn login_page(error: &str) -> String {
    format!(
        r#"<!doctype html><html><body>
<h1>Login</h1>
{error}
<form method="post" action="/login">
<input type="email" name="email" placeholder="Email">
<input type="password" name="password" placeholder="Password">
<button type="submit">Log in</button>
</form>
<p><a href="/register">Register</a></p>
</body></html>"#
    )
}

/// Render the registration page.
fn register_page() -> String {
    r#"<!doctype html><html><body>
<h1>Register</h1>
<form method="post" action="/register">
<input type="email" name="email" placeholder="Email">
<input type="password" name="password" placeholder="Password">
<button type="submit">Create account</button>
</form>
</body></html>"#
        .to_string()
}

/// Render the authenticated dashboard.
fn dashboard_page() -> String {
    r#"<!doctype html><html><body>
<h1>Dashboard</h1>
<p>Welcome back</p>
<a href="/logout">Log out</a>
</body></html>"#
        .to_string()
}

/// `GET /` → the login page.
async fn root() -> Redirect {
    Redirect::to("/login")
}

/// `GET /login` → the login form.
async fn login_form() -> Html<String> {
    Html(login_page(""))
}

/// `POST /login` → set a session cookie and redirect, else re-render with error.
async fn login_submit(State(state): State<AppState>, Form(creds): Form<Credentials>) -> Response {
    match state.login(&creds.email, &creds.password) {
        Some(token) => {
            let cookie = format!("session={token}; Path=/; HttpOnly");
            ([(header::SET_COOKIE, cookie)], Redirect::to("/dashboard")).into_response()
        }
        None => (
            StatusCode::UNAUTHORIZED,
            Html(login_page("These credentials do not match our records.")),
        )
            .into_response(),
    }
}

/// `GET /register` → the registration form.
async fn register_form() -> Html<String> {
    Html(register_page())
}

/// `POST /register` → create the account and redirect to login.
async fn register_submit(
    State(state): State<AppState>,
    Form(creds): Form<Credentials>,
) -> Redirect {
    state.register(&creds.email, &creds.password);
    Redirect::to("/login")
}

/// `GET /dashboard` → the dashboard when authenticated, else redirect to login.
async fn dashboard(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if state.authenticated(&headers) {
        Html(dashboard_page()).into_response()
    } else {
        Redirect::to("/login").into_response()
    }
}

/// `GET /logout` → clear the session cookie and redirect to login.
async fn logout() -> Response {
    let cookie = "session=; Path=/; Max-Age=0".to_string();
    ([(header::SET_COOKIE, cookie)], Redirect::to("/login")).into_response()
}

/// Build the minimal app under test.
fn app() -> Router {
    Router::new()
        .route("/", get(root))
        .route("/login", get(login_form).post(login_submit))
        .route("/register", get(register_form).post(register_submit))
        .route("/dashboard", get(dashboard))
        .route("/logout", get(logout))
        .with_state(AppState::seeded())
}

/// Boot the app and connect a browser, or `None` when no driver is available.
async fn harness() -> Option<(Browser, ServerHandle)> {
    let Some(browser) = Browser::try_connect().await else {
        eprintln!("skipping browser e2e: no WEBDRIVER_URL / reachable driver");
        return None;
    };
    let server = ServerHandle::start(app()).await.expect("server starts");
    Some((browser, server))
}

/// Registration → login → logout round-trip.
#[tokio::test]
#[ignore = "requires WEBDRIVER_URL and a matching chromedriver/geckodriver"]
async fn registration_login_logout_flow() {
    let Some((browser, server)) = harness().await else {
        return;
    };
    let base = server.base_url();
    let result = async {
        browser.visit(&format!("{base}/register")).await?;
        browser.fill("@email", "grace@example.com").await?;
        browser.fill("@password", "s3cret-pass").await?;
        browser.click("button[type=submit]").await?;

        browser.assert_path("/login").await?;
        browser.fill("@email", "grace@example.com").await?;
        browser.fill("@password", "s3cret-pass").await?;
        browser.click("button[type=submit]").await?;

        browser.assert_path("/dashboard").await?;
        browser.assert_see("Dashboard").await?;

        browser.click("a[href='/logout']").await?;
        browser.assert_path("/login").await?;
        Ok::<(), rustasea_testing::browser::BrowserError>(())
    }
    .await;

    browser
        .screenshot_on_failure("registration_login_logout", &result)
        .await;
    browser.close().await.expect("close browser");
    server.shutdown().await;
    result.expect("e2e flow succeeds");
}

/// A wrong password keeps the user on the login page with the error text.
#[tokio::test]
#[ignore = "requires WEBDRIVER_URL and a matching chromedriver/geckodriver"]
async fn wrong_password_shows_validation_error() {
    let Some((browser, server)) = harness().await else {
        return;
    };
    let base = server.base_url();
    let result = async {
        browser.visit(&format!("{base}/login")).await?;
        browser.fill("@email", "ada@example.com").await?;
        browser.fill("@password", "wrong-password").await?;
        browser.click("button[type=submit]").await?;

        browser.assert_see("These credentials do not match").await?;
        browser.assert_path("/login").await?;
        Ok::<(), rustasea_testing::browser::BrowserError>(())
    }
    .await;

    browser
        .screenshot_on_failure("wrong_password", &result)
        .await;
    browser.close().await.expect("close browser");
    server.shutdown().await;
    result.expect("wrong-password case succeeds");
}
