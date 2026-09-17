//! Core application files shared by every variant.
//!
//! Emits the package entry points (`lib.rs`, `main.rs`), environment files, the
//! `askama.toml` template root for server-rendered variants, and the root
//! `.gitignore`/`.gitattributes`/`LICENSE` hygiene files. The `bootstrap/`
//! kernel wiring lives in [`super::bootstrap`] and the developer-tooling config
//! in [`super::tooling`].

use crate::variant::StarterKitVariant;

use super::TemplateFile;

/// Core templates for `variant`.
pub fn entries(variant: StarterKitVariant) -> Vec<TemplateFile> {
    let mut files = vec![
        (".env.example", ENV_EXAMPLE),
        (".gitattributes", GITATTRIBUTES),
        (".gitignore", GITIGNORE),
        ("LICENSE", LICENSE),
        ("README.md", README),
        ("lib.rs", LIB_RS),
        ("main.rs", MAIN_RS),
        ("bootstrap/cache/.gitignore", BOOTSTRAP_CACHE_GITIGNORE),
        ("public/.gitignore", PUBLIC_GITIGNORE),
        ("public/robots.txt", PUBLIC_ROBOTS_TXT),
        ("public/favicon.ico", PUBLIC_FAVICON_ICO),
        ("storage/app/.gitignore", STORAGE_APP_GITIGNORE),
        (
            "storage/app/public/.gitignore",
            STORAGE_APP_PUBLIC_GITIGNORE,
        ),
        (
            "storage/app/private/.gitignore",
            STORAGE_APP_PRIVATE_GITIGNORE,
        ),
        ("storage/logs/.gitignore", STORAGE_LOGS_GITIGNORE),
        ("storage/framework/.gitignore", STORAGE_FRAMEWORK_GITIGNORE),
        (
            "storage/framework/cache/data/.gitignore",
            STORAGE_FRAMEWORK_CACHE_DATA_GITIGNORE,
        ),
        (
            "storage/framework/sessions/.gitignore",
            STORAGE_FRAMEWORK_SESSIONS_GITIGNORE,
        ),
        (
            "storage/framework/testing/.gitignore",
            STORAGE_FRAMEWORK_TESTING_GITIGNORE,
        ),
        (
            "storage/framework/views/.gitignore",
            STORAGE_FRAMEWORK_VIEWS_GITIGNORE,
        ),
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

# --- Broadcasting (config/broadcasting.toml) ---
# The in-process `hub` is the zero-dependency default. Switch to `pusher` or
# `redis` once the matching driver feature and credentials are configured.
BROADCAST_CONNECTION=hub

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

// Normalize line endings on checkout so generated text files are byte-stable
// across platforms; Rust sources keep an explicit `eol=lf` rule.
const GITATTRIBUTES: &str = r##"* text=auto eol=lf
*.rs text eol=lf
"##;

// Generic MIT license; `@@app_pascal@@` names the generated application while
// the copyright holder stays generic because the scaffold has no author input.
const LICENSE: &str = r##"MIT License

Copyright (c) @@app_pascal@@

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"##;

// The `public/` web root mirrors Hypervel: a self-contained `.gitignore` tracks
// the directory while ignoring build output, and the robots/favicon stubs give
// the served root the files a browser or crawler requests first.
const PUBLIC_GITIGNORE: &str = r##"*
!.gitignore
"##;

const PUBLIC_ROBOTS_TXT: &str = r##"User-agent: *
Disallow:
"##;

// Empty placeholder matching Hypervel's 0-byte `public/favicon.ico`.
const PUBLIC_FAVICON_ICO: &str = "";

// The compiled-configuration cache directory (`bootstrap/cache/`) is runtime
// state; track the directory itself and ignore its contents.
const BOOTSTRAP_CACHE_GITIGNORE: &str = r##"*
!.gitignore
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

// Private-disk root (`config/storage.toml` `storage/app/private`); runtime
// contents stay untracked while the directory is preserved.
const STORAGE_APP_PRIVATE_GITIGNORE: &str = r##"*
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

// Per-directory ignores for the framework runtime subdirectories (cache data,
// sessions, testing fixtures, compiled views); each keeps its own `.gitignore`.
const STORAGE_FRAMEWORK_CACHE_DATA_GITIGNORE: &str = r##"*
!.gitignore
"##;

const STORAGE_FRAMEWORK_SESSIONS_GITIGNORE: &str = r##"*
!.gitignore
"##;

const STORAGE_FRAMEWORK_TESTING_GITIGNORE: &str = r##"*
!.gitignore
"##;

const STORAGE_FRAMEWORK_VIEWS_GITIGNORE: &str = r##"*
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

- `app/`: domain actions, concerns, HTTP controllers/middleware/requests, models, providers
- `bootstrap/`: application kernel wiring (providers, commands); `bootstrap/cache/` is runtime config cache
- `config/`: typed TOML configuration, auto-discovered by the loader (`config/*.toml`)
- `routes/`: `web`, `auth`, `settings`, and `console` route tables
- `database/`: migrations, factories, seeders
- `public/`: web root (`robots.txt`, `favicon.ico`); build output is ignored
- `resources/`: presentation layer for the `@@variant@@` variant
- `storage/`: runtime state (`app/private`, `framework/{cache/data,sessions,testing,views}`)
- `tests/`: `feature` and `unit` test suites

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

const ASKAMA_TOML: &str = r##"# askama template root — compiled into the binary at build time.
[general]
dirs = ["resources/views"]
"##;
