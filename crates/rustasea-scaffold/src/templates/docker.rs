//! Docker development environment templates.
//!
//! Emits the container story every generated app ships with, mirroring the
//! `laravel/sail` developer workflow: a multi-stage `Dockerfile` (cargo-chef →
//! slim runtime, non-root user), a `.dockerignore`, a base `docker-compose.yml`
//! (app + postgres/pgvector + redis + minio + mailpit), and a
//! `docker-compose.dev.yml` override adding bind mounts and `cargo-watch`.
//!
//! The templates are raw-string constants per the module convention; the
//! `@@app_name@@` / `@@app_snake@@` placeholders are substituted by
//! [`super::render`]. The generated app is a single crate whose binary is named
//! `@@app_name@@` (see [`super::manifest`]) and which binds `0.0.0.0:8000`, so
//! the image and its healthcheck target that port.

use super::TemplateFile;

/// Docker templates (shared by every variant).
pub fn entries() -> Vec<TemplateFile> {
    vec![
        ("Dockerfile", DOCKERFILE),
        (".dockerignore", DOCKERIGNORE),
        ("docker-compose.yml", DOCKER_COMPOSE),
        ("docker-compose.dev.yml", DOCKER_COMPOSE_DEV),
    ]
}

/// Multi-stage build: cargo-chef dependency caching, `dev` hot-reload stage,
/// and a slim non-root runtime. No secrets are baked in.
const DOCKERFILE: &str = r##"# @@app_pascal@@ — Docker dev image (laravel/sail parity).
#
# Multi-stage cargo-chef build: `planner` records the dependency recipe,
# `builder` cooks it into a cached layer, then only the app source invalidates
# the final `cargo build`. The runtime stage is a slim Debian image running the
# `@@app_name@@` binary as a non-root user. All settings come from the
# environment at runtime — nothing is baked in.

# --- Chef base -------------------------------------------------------------
FROM lukemathwalker/cargo-chef:latest-rust-1.90-bookworm AS chef
WORKDIR /app

# --- Planner: capture the dependency recipe --------------------------------
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# --- Builder: cook dependencies, then build the binary ---------------------
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin @@app_name@@

# --- Dev: hot-reloading stage for the compose dev override -----------------
FROM builder AS dev
RUN cargo install cargo-watch --locked
EXPOSE 8000
CMD ["cargo", "watch", "-x", "run"]

# --- Runtime: slim image with only what the binary reads -------------------
FROM debian:bookworm-slim AS runtime
# `libsqlite3-0` backs the sqlx sqlite driver (system-linked, not bundled) and
# `ca-certificates` lets rustls validate TLS roots.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/*

# Non-root runtime user (uid 1000) matching the compose bind-mount owner.
RUN useradd --create-home --uid 1000 appuser

WORKDIR /app
COPY --from=builder /app/target/release/@@app_name@@ /usr/local/bin/@@app_name@@
# Runtime inputs the binary reads from the working directory.
COPY --from=builder /app/config ./config
COPY --from=builder /app/storage ./storage
RUN mkdir -p storage/logs storage/framework storage/app/public \
    && chown -R appuser:appuser /app

USER appuser
ENV APP_ENV=local
EXPOSE 8000
# Liveness probe: the app has no HTTP health route yet, so check the port.
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD bash -c 'exec 3<>/dev/tcp/127.0.0.1/8000' || exit 1

CMD ["@@app_name@@"]
"##;

/// Build-context exclusions: build output, VCS, local secrets, and runtime junk.
const DOCKERIGNORE: &str = r##"# Docker build context exclusions for @@app_name@@.

# Build output (rebuilt inside the image).
target/

# Version control.
.git/
.gitignore

# Local environment and secrets — never baked into an image.
.env
*.env.local

# Local SQLite runtime databases.
*.sqlite
*.sqlite3
*.sqlite-shm
*.sqlite-wal

# Runtime storage junk (logs, caches, uploaded files).
storage/logs/*
storage/framework/*
storage/app/*
!storage/**/.gitignore

# Editor / OS noise.
.idea/
.vscode/
*.swp
.DS_Store

# Compose files describe the surrounding stack, not the app image.
docker-compose*.yml
"##;

/// Base stack: the app plus Postgres/pgvector, Redis, MinIO, and Mailpit.
const DOCKER_COMPOSE: &str = r##"# @@app_pascal@@ development stack (laravel/sail parity).
#
# `version` is declared so the file also parses under the legacy
# `docker-compose` (v1) binary; the schema is otherwise v2-compatible.
version: "3.8"

services:
  app:
    build:
      context: .
      dockerfile: Dockerfile
    image: @@app_name@@:dev
    restart: unless-stopped
    ports:
      - "8000:8000"
    environment:
      APP_ENV: ${APP_ENV:-local}
      APP_DEBUG: ${APP_DEBUG:-true}
      APP_URL: ${APP_URL:-http://localhost:8000}
      APP_KEY: ${APP_KEY:-}
      # Full connection URL wins over the granular DB_* fields at runtime.
      DATABASE_URL: ${DATABASE_URL:-postgres://rustasea:secret@postgres:5432/rustasea}
      DB_CONNECTION: ${DB_CONNECTION:-postgres}
      DB_HOST: ${DB_HOST:-postgres}
      DB_PORT: ${DB_PORT:-5432}
      DB_DATABASE: ${DB_DATABASE:-rustasea}
      DB_USERNAME: ${DB_USERNAME:-rustasea}
      DB_PASSWORD: ${DB_PASSWORD:-secret}
      REDIS_URL: ${REDIS_URL:-redis://redis:6379/0}
      REDIS_HOST: ${REDIS_HOST:-redis}
      REDIS_PORT: ${REDIS_PORT:-6379}
      CACHE_PREFIX: ${CACHE_PREFIX:-rustasea-cache-}
      QUEUE_CONNECTION: ${QUEUE_CONNECTION:-database}
      SESSION_DRIVER: ${SESSION_DRIVER:-memory}
      MAIL_MAILER: ${MAIL_MAILER:-smtp}
      MAIL_HOST: ${MAIL_HOST:-mailpit}
      MAIL_PORT: ${MAIL_PORT:-1025}
      MAIL_FROM_ADDRESS: ${MAIL_FROM_ADDRESS:-hello@example.com}
      # Object storage backed by MinIO (S3-compatible); enable the
      # `rustasea-storage/aws` feature and the `s3` disk to use it.
      AWS_ACCESS_KEY_ID: ${MINIO_ROOT_USER:-rustasea}
      AWS_SECRET_ACCESS_KEY: ${MINIO_ROOT_PASSWORD:-rustasea-secret}
      AWS_DEFAULT_REGION: ${AWS_DEFAULT_REGION:-us-east-1}
      AWS_BUCKET: ${AWS_BUCKET:-rustasea}
      AWS_ENDPOINT: ${AWS_ENDPOINT:-http://minio:9000}
      LOG_CHANNEL: ${LOG_CHANNEL:-stack}
      LOG_LEVEL: ${LOG_LEVEL:-debug}
    depends_on:
      postgres:
        condition: service_healthy
      redis:
        condition: service_healthy
      minio:
        condition: service_healthy
      mailpit:
        condition: service_healthy
    networks:
      - rustasea

  postgres:
    image: pgvector/pgvector:pg16
    restart: unless-stopped
    ports:
      - "5432:5432"
    environment:
      POSTGRES_DB: ${DB_DATABASE:-rustasea}
      POSTGRES_USER: ${DB_USERNAME:-rustasea}
      POSTGRES_PASSWORD: ${DB_PASSWORD:-secret}
    volumes:
      - postgres-data:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U ${DB_USERNAME:-rustasea} -d ${DB_DATABASE:-rustasea}"]
      interval: 10s
      timeout: 5s
      retries: 5
      start_period: 10s
    networks:
      - rustasea

  redis:
    image: redis:7-alpine
    restart: unless-stopped
    ports:
      - "6379:6379"
    volumes:
      - redis-data:/data
    healthcheck:
      test: ["CMD", "redis-cli", "ping"]
      interval: 10s
      timeout: 5s
      retries: 5
      start_period: 5s
    networks:
      - rustasea

  minio:
    image: minio/minio
    restart: unless-stopped
    command: server /data --console-address ":9001"
    ports:
      - "9000:9000"
      - "9001:9001"
    environment:
      MINIO_ROOT_USER: ${MINIO_ROOT_USER:-rustasea}
      MINIO_ROOT_PASSWORD: ${MINIO_ROOT_PASSWORD:-rustasea-secret}
    volumes:
      - minio-data:/data
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:9000/minio/health/live"]
      interval: 10s
      timeout: 5s
      retries: 5
      start_period: 10s
    networks:
      - rustasea

  mailpit:
    image: axllent/mailpit
    restart: unless-stopped
    ports:
      - "1025:1025"
      - "8025:8025"
    environment:
      MP_SMTP_AUTH_ACCEPT_ANY: "1"
      MP_SMTP_AUTH_ALLOW_INSECURE: "1"
    healthcheck:
      test: ["CMD", "wget", "-q", "--spider", "http://localhost:8025/"]
      interval: 10s
      timeout: 5s
      retries: 5
      start_period: 5s
    networks:
      - rustasea

volumes:
  postgres-data:
  redis-data:
  minio-data:

networks:
  rustasea:
    driver: bridge
"##;

/// Dev override: bind-mounted sources + `cargo-watch` hot reload.
const DOCKER_COMPOSE_DEV: &str = r##"# @@app_pascal@@ development override — hot-reload the app.
#
# Layer on top of the base stack:
#
#   docker-compose -f docker-compose.yml -f docker-compose.dev.yml up
#
# Named volumes keep the build cache and cargo caches off the bind mount so
# they survive restarts.
version: "3.8"

services:
  app:
    build:
      context: .
      dockerfile: Dockerfile
      target: dev
    image: @@app_name@@:dev-watch
    command: cargo watch -x run
    environment:
      RUST_LOG: ${RUST_LOG:-debug}
      APP_ENV: ${APP_ENV:-local}
      APP_DEBUG: ${APP_DEBUG:-true}
    volumes:
      - .:/app
      - cargo-target:/app/target
      - cargo-registry:/usr/local/cargo/registry
      - cargo-git:/usr/local/cargo/git

volumes:
  cargo-target:
  cargo-registry:
  cargo-git:
"##;
