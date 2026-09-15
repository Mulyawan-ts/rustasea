//! Core application files shared by every variant.
//!
//! Emits the package entry points (`lib.rs`, `main.rs`), environment files, the
//! `askama.toml` template root for server-rendered variants, and the
//! `bootstrap/` kernel wiring that was an empty placeholder before the
//! starter kit existed (`bootstrap/providers.rs`, `bootstrap/commands.rs`).

use crate::variant::StarterKitVariant;

use super::TemplateFile;

/// Core templates for `variant`.
pub fn entries(variant: StarterKitVariant) -> Vec<TemplateFile> {
    let mut files = vec![
        (".env.example", ENV_EXAMPLE),
        (".gitignore", GITIGNORE),
        ("README.md", README),
        ("lib.rs", LIB_RS),
        ("main.rs", MAIN_RS),
        ("bootstrap/mod.rs", BOOTSTRAP_MOD),
        ("bootstrap/app.rs", BOOTSTRAP_APP),
        ("bootstrap/providers.rs", BOOTSTRAP_PROVIDERS),
        ("bootstrap/commands.rs", BOOTSTRAP_COMMANDS),
        ("storage/app/.gitignore", STORAGE_APP_GITIGNORE),
        (
            "storage/app/public/.gitignore",
            STORAGE_APP_PUBLIC_GITIGNORE,
        ),
        ("storage/logs/.gitignore", STORAGE_LOGS_GITIGNORE),
        ("storage/framework/.gitignore", STORAGE_FRAMEWORK_GITIGNORE),
        ("storage/archive/.gitignore", STORAGE_ARCHIVE_GITIGNORE),
    ];
    if variant.uses_askama() {
        files.push(("askama.toml", ASKAMA_TOML));
    }
    files
}

const ENV_EXAMPLE: &str = r##"# Copy to `.env` and adjust per environment. Environment variables always win
# over `config/*.toml` (the loader applies the process environment last; nested
# keys use the `__` separator). Every variable below is read by a generated
# config file or its typed consumer.

# --- Application (config/app.toml) ---
APP_NAME=@@app_name@@
APP_ENV=local
APP_DEBUG=true
APP_URL=http://localhost:3000
APP_KEY=
APP_LOCALE=en
APP_FALLBACK_LOCALE=en
APP_MAINTENANCE_DRIVER=file
APP_MAINTENANCE_STORE=database

# --- Logging (config/logging.toml) ---
LOG_CHANNEL=stack
LOG_LEVEL=debug
LOG_STACK=daily,stderr
LOG_DAILY_DAYS=14

# --- Mail (config/mail.toml) ---
MAIL_MAILER=log
MAIL_HOST=127.0.0.1
MAIL_PORT=2525
MAIL_USERNAME=
MAIL_PASSWORD=
MAIL_FROM_ADDRESS=hello@example.com
MAIL_FROM_NAME=@@app_pascal@@

# --- Cache (config/cache.toml) ---
CACHE_PREFIX=rustasea-cache-

# --- Queue (config/queue.toml) ---
QUEUE_CONNECTION=database

# --- Session (config/session.toml) ---
# Browser starter kits authenticate with session cookies + CSRF.
SESSION_DRIVER=memory
SESSION_LIFETIME=120
SESSION_COOKIE=rustasea-session
# `SESSION_SECURE_COOKIE` is accepted as an alias of `SESSION_SECURE` (Laravel
# / starter-kit naming); when both are set, the explicit `SESSION_SECURE` wins.
SESSION_SECURE=false
SESSION_SECURE_COOKIE=false
SESSION_SAME_SITE=lax
SESSION_EXPIRE_ON_CLOSE=false
SESSION_ENCRYPT=false
SESSION_PARTITIONED_COOKIE=false
SESSION_HTTP_ONLY=true
SESSION_CONNECTION=default
SESSION_TABLE=sessions
SESSION_STORE=default
SESSION_PATH=/
SESSION_DOMAIN=

# --- Auth (config/auth.toml) ---
AUTH_GUARD=web
AUTH_PASSWORD_BROKER=users
AUTH_MODEL=App\Models\User
AUTH_PASSWORD_RESET_TOKEN_TABLE=password_reset_tokens
# Seconds before a sensitive action re-confirms the password (Laravel parity).
AUTH_PASSWORD_TIMEOUT=10800

# --- Fortify (config/fortify.toml) ---
# Passkeys are inert parity today; the secret falls back to APP_KEY when blank.
PASSKEYS_USER_HANDLE_SECRET=

# --- Database (config/database.toml) ---
DB_CONNECTION=sqlite
DB_URL=sqlite://database.sqlite?mode=rwc
DATABASE_URL=
DB_HOST=127.0.0.1
DB_PORT=5432
DB_DATABASE=database/database.sqlite
DB_USERNAME=rustasea
DB_PASSWORD=secret

# --- Redis (config/database.toml + config/cache.toml) ---
REDIS_URL=redis://127.0.0.1:6379/0
REDIS_HOST=127.0.0.1
REDIS_PORT=6379
REDIS_USERNAME=
REDIS_PASSWORD=
REDIS_DB=0
REDIS_CACHE_DB=1

# --- MongoDB (config/mongo.toml + config/database.toml) ---
MONGODB_URI=mongodb://localhost:27017
MONGODB_DATABASE=rustasea

# --- Third-party services (config/services.toml + config/storage.toml) ---
# NEVER commit real secrets.
POSTMARK_API_KEY=
RESEND_API_KEY=
AWS_ACCESS_KEY_ID=
AWS_SECRET_ACCESS_KEY=
AWS_DEFAULT_REGION=us-east-1
AWS_BUCKET=
SLACK_BOT_USER_OAUTH_TOKEN=
SLACK_BOT_USER_DEFAULT_CHANNEL=

# --- Docker dev stack (docker-compose.yml + docker-compose.dev.yml) ---
# Only read by the compose files, not the framework runtime. Defaults are inline
# in docker-compose.yml, so this section is optional — uncomment to override.
# DB_HOST=postgres
# DB_PORT=5432
# REDIS_URL=redis://redis:6379/0
# MAIL_MAILER=smtp
# MAIL_HOST=mailpit
# MAIL_PORT=1025
# MINIO_ROOT_USER=rustasea
# MINIO_ROOT_PASSWORD=rustasea-secret
# AWS_ENDPOINT=http://minio:9000
# RUST_LOG=debug
"##;

const GITIGNORE: &str = r##"/target
/.env
/database/*.sqlite
"##;

// Storage layout uses the laravel/livewire-starter-kit convention: each runtime
// directory ships its own self-contained `.gitignore` so the directory itself is
// tracked while its runtime contents are ignored. This keeps the generated tree
// aligned with the RustaSea repo root without a central storage negation block.
const STORAGE_APP_GITIGNORE: &str = r##"*
!public/
!.gitignore
"##;

const STORAGE_APP_PUBLIC_GITIGNORE: &str = r##"*
!.gitignore
"##;

const STORAGE_LOGS_GITIGNORE: &str = r##"*
!.gitignore
"##;

// The `down` file is the maintenance-mode marker consumed by
// `rustasea-foundation`'s `MAINTENANCE_MARKER` (`storage/framework/down`).
const STORAGE_FRAMEWORK_GITIGNORE: &str = r##"# Maintenance-mode marker (rustasea-foundation MAINTENANCE_MARKER); all other
# storage/framework content is runtime state and must stay untracked.
*
!.gitignore
"##;

const STORAGE_ARCHIVE_GITIGNORE: &str = r##"*
!.gitignore
"##;

const README: &str = r##"# @@app_pascal@@

A RustaSea starter kit generated with:

```sh
cargo rustasea new @@app_name@@ --variant @@variant@@
```

## Layout

- `app/` — domain actions, concerns, HTTP controllers/middleware/requests, models, providers
- `bootstrap/` — application kernel wiring (providers, commands)
- `config/` — typed TOML configuration, auto-discovered by the loader (`config/*.toml`)
- `routes/` — `web`, `auth`, `settings`, and `console` route tables
- `database/` — migrations, factories, seeders
- `resources/` — presentation layer for the `@@variant@@` variant
- `tests/` — `feature` and `unit` test suites

## Development

```sh
cargo run
```

The server binds `0.0.0.0:3000` by default (`APP_URL` overrides it).

## Docker

A container stack with laravel/sail parity is generated alongside the app:

```bash
docker-compose up -d                                          # app + infra
docker-compose -f docker-compose.yml -f docker-compose.dev.yml up  # hot reload
docker-compose down                                           # tear down
```

| Service | Ports | Purpose |
|---|---|---|
| `app` | `3000` | The @@app_pascal@@ HTTP app |
| `postgres` | `5432` | Primary SQL store + pgvector |
| `redis` | `6379` | Cache + queue backend |
| `minio` | `9000`, `9001` | S3-compatible storage (`9001` = console) |
| `mailpit` | `1025`, `8025` | SMTP capture (`1025`) + web UI (`8025`) |

App: <http://localhost:3000> · Mailpit UI: <http://localhost:8025> ·
MinIO console: <http://localhost:9001>.
"##;

const LIB_RS: &str = r##"//! @@app_pascal@@ — RustaSea application library.
//!
//! The module tree mirrors the Laravel layout: `app/` holds domain actions,
//! concerns, HTTP, models, and providers; `bootstrap/` wires the kernel;
//! `routes/` owns the route tables; `database/` holds migrations, factories,
//! and seeders.

pub mod app;
pub mod bootstrap;
pub mod database;
pub mod routes;
"##;

const MAIN_RS: &str = r##"//! @@app_pascal@@ HTTP entry point.

use std::sync::Arc;

use @@app_snake@@::{bootstrap, routes};
use rustasea::http::AppState;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure the container and run the provider boot DAG. `configure`
    // returns `Result` so a misconfigured boot aborts before the server starts
    // instead of silently ignoring the failure.
    let app = bootstrap::app::configure()?;

    // Create the shared HTTP state once; every route table receives this same
    // instance so handlers resolve one application state, not a disconnected one.
    let state = Arc::new(AppState::new("local", true));

    // Build the axum router from the generated route tables.
    let router = routes::router(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    println!("@@app_pascal@@ listening on http://0.0.0.0:3000");

    axum::serve(listener, router)
        .with_graceful_shutdown(app.shutdown())
        .await?;
    Ok(())
}
"##;

const BOOTSTRAP_MOD: &str = r##"//! Application bootstrap — providers, commands, and kernel configuration.

pub mod app;
pub mod commands;
pub mod providers;
"##;

const BOOTSTRAP_APP: &str = r##"//! Application bootstrap — `Application::configure` for @@app_pascal@@.
//!
//! Registers the generated service providers and runs the register → boot DAG
//! before the HTTP kernel starts serving.

use rustasea::foundation::BootError;
use rustasea::Application;

use crate::bootstrap::providers;

/// Build and boot the application container.
///
/// Returns [`BootError`] when the provider graph contains a cycle or an
/// unresolved dependency, so a misconfigured boot never starts the server.
pub fn configure() -> Result<Application, BootError> {
    let mut app = Application::configure(|_| {});
    for provider in providers::providers() {
        app.provider(provider);
    }
    app.boot()?;
    Ok(app)
}
"##;

const BOOTSTRAP_PROVIDERS: &str = r##"//! Provider registry — service providers registered by the application.
//!
//! This registry is populated by the starter kit (previously empty) and is the
//! registration site for providers generated with `cargo rustasea make:provider`.

use rustasea::ServiceProvider;

use crate::app::providers::{AppServiceProvider, AuthServiceProvider};

/// Providers wired into the boot DAG, in registration order.
///
/// Order is the tie-breaker for providers without `dependencies()`; the
/// foundation `Application::boot` topologically sorts them regardless.
pub fn providers() -> Vec<Box<dyn ServiceProvider>> {
    vec![Box::new(AppServiceProvider), Box::new(AuthServiceProvider)]
}
"##;

const BOOTSTRAP_COMMANDS: &str = r##"//! CLI command registry — `cargo artisan` console commands.
//!
//! This registry gives the previously-empty command site a real home; commands
//! generated into `app/console/commands/*` are appended here.

/// Names of the console commands registered for the application.
pub fn commands() -> Vec<&'static str> {
    vec![]
}
"##;

const ASKAMA_TOML: &str = r##"# askama template root — compiled into the binary at build time.
[general]
dirs = ["resources/views"]
"##;
