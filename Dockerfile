# RustaSea workspace dev image (laravel/sail parity).
#
# Multi-stage build using the cargo-chef pattern so the dependency graph is
# compiled in its own cached layer: `planner` records the dependency recipe,
# `builder` cooks it, then only the workspace source changes invalidate the
# final `cargo build`. The runtime stage is a slim Debian image running the
# `rustasea-app` binary as a non-root user. No secrets are baked in — every
# setting is supplied at runtime through the environment (see docker-compose).

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
# Compile only the dependency graph the `rustasea-app` binary needs.
RUN cargo chef cook --release --recipe-path recipe.json --bin rustasea-app
COPY . .
RUN cargo build --release --bin rustasea-app

# --- Dev: hot-reloading stage for the compose dev override -----------------
# Bind-mounted over the source tree; cargo-watch rebuilds on change.
FROM builder AS dev
RUN cargo install cargo-watch --locked
EXPOSE 8000
CMD ["cargo", "watch", "-x", "run -p rustasea-app"]

# --- Runtime: slim image with only what the binary reads -------------------
FROM debian:bookworm-slim AS runtime
# `libsqlite3-0` backs the sqlx sqlite driver (linked against the system
# library, not bundled); `ca-certificates` lets rustls validate TLS roots and
# `curl` powers the container healthcheck.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/*

# Non-root runtime user (uid 1000) matching the compose bind-mount owner.
RUN useradd --create-home --uid 1000 appuser

WORKDIR /app
COPY --from=builder /app/target/release/rustasea-app /usr/local/bin/rustasea-app
# Runtime inputs the binary reads from the working directory: typed config,
# runtime templates, and the writable storage tree.
COPY --from=builder /app/config ./config
COPY --from=builder /app/resources ./resources
COPY --from=builder /app/storage ./storage
RUN mkdir -p storage/logs storage/framework storage/app/public \
    && chown -R appuser:appuser /app

USER appuser
ENV APP_ENV=local
EXPOSE 8000
# Liveness probe against the framework `/health` route.
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl -fsS http://localhost:8000/health || exit 1

CMD ["rustasea-app"]
