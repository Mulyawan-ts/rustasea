//! Web routes — landing, health, welcome, and the authenticated dashboard.
//!
//! `/`, `/health`, and `/welcome` are ungated and keep their original
//! behaviour; `/dashboard` is the authenticated surface, gated by the `auth`
//! and `verified` middleware ids registered in [`super`].
//!
//! Every HTML page is rendered from `resources/views/*.html` through the
//! runtime minijinja engine ([`MinijinjaEngine`]) rather than a Rust string
//! literal, so the templates the app ships are the templates it serves.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use axum::extract::Extension;
use axum::response::Response;
use serde::Serialize;

use rustasea::auth::AuthUser;
use rustasea::http::AppState;
use rustasea::router::Router as RouteTable;
use rustasea::view::{MinijinjaEngine, ViewEngine, ViewError, ViewResponse};

use super::{AUTH, VERIFIED};

/// Build the shared runtime template engine once, rooted at the views directory.
///
/// The framework default root ([`rustasea::view::VIEWS_DIR`] = `resources/views`)
/// is resolved against the process working directory, which is correct for a
/// deployed app and for `cargo run` from the workspace root. `cargo test -p
/// rustasea-app` instead runs with the crate root as its working directory, so
/// when the default root is absent the engine falls back to the workspace-root
/// `resources/views` derived from `CARGO_MANIFEST_DIR`; both paths render the
/// same on-disk templates.
fn engine() -> &'static MinijinjaEngine {
    static ENGINE: OnceLock<MinijinjaEngine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        if Path::new(rustasea::view::VIEWS_DIR).is_dir() {
            MinijinjaEngine::from_default_root()
        } else {
            MinijinjaEngine::new(super::resources_root().join("views"))
        }
    })
}

/// Serializable projection of [`AuthUser`] exposed to templates as `user`.
///
/// Only the fields a view renders are projected, so the template context stays
/// a small, explicit contract rather than the guard's full principal.
#[derive(Serialize)]
struct NavUser {
    /// Primary key of the authenticated record.
    id: String,
    /// Login identifier, when the guard exposes one.
    email: Option<String>,
    /// Guard name that produced the principal.
    guard: String,
}

impl From<AuthUser> for NavUser {
    fn from(user: AuthUser) -> Self {
        Self {
            id: user.id,
            email: user.email,
            guard: user.guard,
        }
    }
}

/// View context shared by every page: the optional authenticated principal.
#[derive(Serialize)]
struct ViewContext {
    /// Authenticated principal, or `None` for an anonymous visitor.
    user: Option<NavUser>,
}

/// Render `template` with the optional authenticated principal as `user`.
fn render_view(template: &str, user: Option<AuthUser>) -> Result<ViewResponse, ViewError> {
    let context = ViewContext {
        user: user.map(NavUser::from),
    };
    engine().render(template, &context)
}

/// Register the web route table onto `table`.
///
/// `/dashboard` is registered inside a [`RouteTable::group`] because
/// [`RouteTable::middleware`] is sticky — its pending middleware applies to
/// every subsequent route on the same table. A group gives the gate its own
/// scope so it cannot leak onto routes registered later (auth, settings,
/// console).
pub fn register(table: &mut RouteTable) {
    table.get_action("/", index).named("home");
    table.get_action("/health", health);
    table.get_action("/welcome", index);
    table.group(|group| {
        group
            .middleware(AUTH)
            .middleware(VERIFIED)
            .get_action("/dashboard", dashboard)
            .named("dashboard");
    });
}

/// GET / and /welcome — serve the Laravel-style welcome page.
async fn index(user: Option<Extension<AuthUser>>) -> Result<ViewResponse, ViewError> {
    render_view("welcome.html", user.map(|Extension(user)| user))
}

/// Health check response body.
#[derive(serde::Serialize)]
struct Health {
    /// Liveness indicator.
    status: &'static str,
    /// Service name.
    service: &'static str,
    /// Current application environment.
    env: String,
}

/// GET /health — JSON liveness probe with HTTP status.
async fn health(Extension(state): Extension<Arc<AppState>>) -> Response {
    rustasea::http::JsonResponse::ok(Health {
        status: "ok",
        service: "rustasea-app",
        env: state.env.clone(),
    })
}

/// GET /dashboard — authenticated page behind the `auth` gate.
async fn dashboard(user: Option<Extension<AuthUser>>) -> Result<ViewResponse, ViewError> {
    render_view("dashboard.html", user.map(|Extension(user)| user))
}
