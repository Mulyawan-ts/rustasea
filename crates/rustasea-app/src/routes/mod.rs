//! Route tables for the runnable `rustasea-app` binary.
//!
//! Concern-scoped tables mirror Laravel's `routes/{web,auth,settings,console}`
//! split (ADR-0002 decision 9). Each module registers its routes onto the
//! shared [`RouteTable`] via a `register(&mut RouteTable)` function; this
//! module merges them, registers the named middleware the tables declare, and
//! compiles the result into a dispatchable axum router.
//!
//! The workspace-root `routes/web.rs` is **not** compiled (see that file's
//! shim header); the canonical route definitions live here.

pub mod auth;
pub mod console;
pub mod settings;
pub mod web;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::Request;
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tower_http::services::ServeDir;

use rustasea::auth::AuthUser;
use rustasea::http::AppState;
use rustasea::router::Router as RouteTable;

/// Middleware id: require an authenticated principal (`Extension<AuthUser>`).
pub const AUTH: &str = "auth";
/// Middleware id: require a verified email address.
pub const VERIFIED: &str = "verified";
/// Middleware id: require recent password confirmation.
pub const PASSWORD_CONFIRM: &str = "password.confirm";

/// Where unauthenticated requests to a gated route are redirected.
pub const LOGIN_PATH: &str = "/login";

/// Build the full route table for the application.
///
/// Registers every concern table plus the named middleware those tables
/// declare, so [`RouteTable::try_into_axum_router`] never fails closed with
/// [`rustasea::router::RouteError::UnknownMiddleware`].
pub fn table() -> RouteTable {
    let mut table = RouteTable::new();
    register_middleware(&mut table);
    web::register(&mut table);
    auth::register(&mut table);
    settings::register(&mut table);
    console::register(&mut table);
    table
}

/// Compile an already-built route table into a servable axum router.
///
/// The shared [`AppState`] is injected as a request extension
/// (`Extension<Arc<AppState>>`) for every route, matching the crate-local
/// `Arc<AppState>` threading contract. A build failure (an unregistered
/// middleware id) fails closed with a `500` router rather than panicking, so
/// the served router never silently drops a declared gate.
///
/// `main` builds the table once, prints it, then calls this with the same
/// table, so the printed route table is exactly the one served. Tests use
/// [`table`] + this to build a router from a known table.
pub fn compile(mut table: RouteTable, state: Arc<AppState>) -> axum::Router {
    table.layer(axum::Extension(state));
    match table.try_into_axum_router() {
        Ok(router) => mount_assets(router),
        Err(error) => {
            eprintln!("route table build failed: {error}");
            axum::Router::new().fallback(|| async { StatusCode::INTERNAL_SERVER_ERROR })
        }
    }
}

/// Mount the static asset service at `/assets/*`, serving the workspace
/// `resources/` directory.
///
/// [`ServeDir`] owns all path handling: it percent-decodes the request path,
/// rejects any segment containing `..` (or a backslash), and returns `404`
/// rather than escaping the root, so traversal is handled by the library and
/// never hand-rolled. The route is mounted on the already-compiled router
/// outside any [`RouteTable::group`], so it inherits no authentication gate,
/// and the `/assets` prefix cannot shadow the existing named routes (`/`,
/// `/health`, `/dashboard`, `/settings/*`, `/console`).
fn mount_assets(router: axum::Router) -> axum::Router {
    router.nest_service("/assets", ServeDir::new(resources_root()))
}

/// Locate the workspace `resources/` directory.
///
/// Prefers the process-relative `resources/` — correct for a deployed app and
/// for `cargo run` from the workspace root — and otherwise falls back to the
/// workspace root derived from `CARGO_MANIFEST_DIR` (the crate lives at
/// `crates/rustasea-app`, two levels below it). The fallback keeps `cargo test`
/// working, since tests run with the crate root as their working directory.
pub(crate) fn resources_root() -> PathBuf {
    let cwd_relative = PathBuf::from("resources");
    if cwd_relative.is_dir() {
        return cwd_relative;
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("resources")
}

/// Minimal `501 Not Implemented` response for flows that are not wired yet.
///
/// Shared by the concern tables so an unimplemented POST flow returns a clear,
/// non-panicking response instead of a `todo!()`.
pub(crate) fn not_implemented(flow: &str) -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        format!("{flow} is not implemented in this scaffold yet."),
    )
        .into_response()
}

/// Register the named middleware the concern tables declare.
fn register_middleware(table: &mut RouteTable) {
    table.register_middleware(AUTH, |method_router| {
        method_router.layer(axum::middleware::from_fn(require_authenticated))
    });
    table.register_middleware(VERIFIED, |method_router| {
        method_router.layer(axum::middleware::from_fn(require_verified))
    });
    table.register_middleware(PASSWORD_CONFIRM, |method_router| {
        method_router.layer(axum::middleware::from_fn(require_password_confirmed))
    });
}

/// `auth` guard — reject requests without an authenticated principal.
///
/// Identity travels as a request extension (`Extension<AuthUser>`), the
/// convention documented by the stateless `SessionGuard`/`JwtGuard` in
/// `rustasea-auth`: the guard holds no per-request state, so the HTTP layer
/// projects `Guard::parse` results into `Extension<AuthUser>`. A request
/// lacking that extension is answered with `302 Found` → [`LOGIN_PATH`] (the
/// web-app convention, so a browser is sent to the login form) rather than
/// `401`. This app does not yet wire a session layer that inserts the
/// extension, so gated routes redirect until that lands — an honest gate, not
/// a faked login.
async fn require_authenticated(request: Request, next: Next) -> Response {
    if request.extensions().get::<AuthUser>().is_some() {
        next.run(request).await
    } else {
        redirect_to_login()
    }
}

/// `verified` guard — email-verification gate.
///
/// **Placeholder (documented).** The authenticated principal type (`AuthUser`)
/// exposes no verification state, so email verification cannot be enforced
/// here yet. The guard therefore requires only an authenticated principal —
/// the strongest check the current principal supports — and documents the gap
/// rather than silently letting unverified users through. Swap in a real
/// `email_verified_at` check once the principal carries it.
async fn require_verified(request: Request, next: Next) -> Response {
    if request.extensions().get::<AuthUser>().is_some() {
        next.run(request).await
    } else {
        redirect_to_login()
    }
}

/// `password.confirm` guard — recent-password-confirmation gate.
///
/// **Placeholder (documented).** The session guard stores no
/// password-confirmation timestamp and the principal exposes none, so recent
/// confirmation cannot be checked; like [`require_verified`], the guard
/// degrades to requiring an authenticated principal and states the gap
/// honestly.
async fn require_password_confirmed(request: Request, next: Next) -> Response {
    if request.extensions().get::<AuthUser>().is_some() {
        next.run(request).await
    } else {
        redirect_to_login()
    }
}

/// Build the `302 Found` → [`LOGIN_PATH`] redirect response.
fn redirect_to_login() -> Response {
    (
        StatusCode::FOUND,
        [(header::LOCATION, HeaderValue::from_static(LOGIN_PATH))],
    )
        .into_response()
}
