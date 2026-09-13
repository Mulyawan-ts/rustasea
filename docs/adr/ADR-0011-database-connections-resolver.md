# ADR-0011 — Config-Driven Named Database Connections and Resolver

> **Status:** Accepted
> **Date:** 2026-09-12
> **Deciders:** Tech Lead, Backend
> **Milestone:** M2 (DB agnostic)
> **Related:** ADR-0004 (sqlx primary) · ADR-0006 (workspace crates) · DB-001 · FSD FS-M2-01 · `crates/rustasea-orm/src/db.rs` · `config/database.toml`

## Context

`config/database.toml` was flat: a single `url` plus an optional `driver` field that was **declared but never consumed** (`crates/rustasea-orm/src/db.rs` dispatched purely on the URL scheme). There was no way to declare multiple named databases, no granular `host`/`port`/`username`/`password` fields, and no default selector — switching pgsql/mysql/sqlite required editing the one `url` and recompiling nothing but the config.

Two independent consumers had already duplicated the resolution logic with subtly different precedence: the CLI (`crates/rustasea-cli/src/commands/ops/migration.rs`, env-first) and `xtask` (`xtask/src/migrate.rs`, config-first). A Laravel-parity target requires `database.default` plus `[database.connections.<name>]` with a resolver, without breaking the existing single-URL contract (`DATABASE_URL` > `DATABASE__URL` > config).

Constraints: BR-08 incremental adoption (legacy configs must keep working); no new dependency cycles (ADR-0006); typed errors over panics; files ≤500 lines; the ORM owns database config parsing.

## Decision

**Add a config-driven named-connection surface owned by `rustasea-orm`, with a lazily-connected, cached `ConnectionResolver`, while preserving the legacy flat `url` and environment precedence exactly.**

1. **Config surface** — `[database]` gains `default` and `[database.connections.<name>]` tables. A connection declares a `driver` (selector + validator) plus either a full `url` or granular `host`/`port`/`database`/`username`/`password`/`charset`. Pool tuning lives in `[database.pool]` (shared) or `[….connections.<name>.pool]` (override).
2. **Typed config** — `DatabaseConfig { default, driver, url, pool, connections: BTreeMap<String, ConnectionConfig> }` and `ConnectionConfig::build_url()` in the new `crates/rustasea-orm/src/connections.rs`, parsed through `rustasea-config`'s `ConfigLoader`. `BTreeMap` gives deterministic iteration.
3. **Resolver** — `ConnectionResolver` caches one `DbPool` per name behind a `tokio::sync::Mutex`; `resolve(None)` honours `default`, `resolve(Some(name))` selects explicitly, and the first call connects lazily.
4. **Backward compatibility** — when `[database.connections]` is absent, `DatabaseConfig::synthesize` folds the legacy flat `url` into one implicit `default` connection. CLI and `xtask` keep `DATABASE_URL`/`DATABASE__URL` ahead of config, so single-connection apps behave identically.
5. **`driver` consumed** — cross-checked against the URL scheme; a mismatch is `ConnectionError::DriverMismatch`, an unknown driver `ConnectionError::UnsupportedDriver`.
6. **Typed errors** — a new `ConnectionError` enum (`UnknownConnection`, `NotConfigured`, `InvalidConfig`, `UnsupportedDriver`, `MissingField`, `DriverMismatch`) is surfaced through `OrmError::Connection`.

## Alternatives

| Option | Pros | Cons | Verdict |
|--------|------|------|---------|
| **Named connections + resolver in `rustasea-orm` (chosen)** | ORM already owns `DbPool`; one resolution path for CLI/xtask/app; legacy `url` synthesized | Slightly larger ORM surface | **Chosen** |
| Keep flat config, add a second optional `url` | Minimal change | No naming, no default selector, no granular fields — fails Laravel parity | Rejected |
| Put the resolver in `rustasea-config` | Config crate stays generic | `rustasea-config` must depend on `rustasea-orm` for `DbPool` → **dependency cycle** (ADR-0006) | Rejected |
| New `rustasea-db-config` crate | Clean isolation | Adds a crate for ~one module; CLI/xtask would import two crates | Rejected |
| Eagerly connect every connection at boot | No lazy-state | Boots fail when an unused secondary DB is down; slower startup | Rejected |

## Consequences

- `cargo test -p rustasea-orm` covers parse-three-connections, per-connection URL resolution, default switching, granular URL building, and the negative cases (unknown name, missing field, driver/url mismatch, unsupported driver).
- `DbPool` gains `connect_with_settings` / `PoolSettings`; `DbPool::connect` keeps its exact previous behaviour (settings default to `min=0`, `max=10`, 30s acquire timeout, no idle reaping).
- `rustasea-orm` now depends on `rustasea-config` (no cycle: config depends only on `config`/`serde`/`dotenvy`).
- Positive: one connection-resolution implementation shared by CLI, `xtask`, and future app bootstrap; config-only driver switching.
- Negative: `config/database.toml` documents two shapes (flat + named); mitigated by `synthesize` keeping them compatible and by the ADR.
- Neutral: `[database.connections]` values are not currently overridable per-connection via env overlays beyond the whole-`url` env keys; a future ADR can extend the overlay if needed.
