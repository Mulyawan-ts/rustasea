# ADR-0010 — MongoDB Document Support via a Feature-Gated `rustasea-mongo` Crate

> **Status:** Accepted
> **Date:** 2026-09-12
> **Deciders:** Tech Lead, Backend
> **Milestone:** M7 (DB Agnostic follow-up)
> **Related:** FR-200 (multi-driver) · ADR-0004 (sqlx primary, sea-orm optional) · ADR-0006 (workspace crate boundaries) · ADR-0008 (feature-flagged optional integrations) · task DB-004

## Context

RustaSea's persistence layer is SQL-first: `rustasea-orm` wraps `sqlx` with a
query builder, migrations, and a `Model` contract (ADR-0004). Users have asked
for a popular non-relational store, specifically MongoDB, whose document model
does not map cleanly onto the relational query builder.

Two pressures shaped the decision:

- **Semantic mismatch.** A MongoDB document is a schemaless BSON value with
  nested arrays/objects and an `_id` primary key. Forcing it through the SQL
  ORM's column/table abstractions would either leak relational concepts into
  documents or require a parallel, half-implemented abstraction inside
  `rustasea-orm`.
- **Pay-for-what-you-use.** The official `mongodb` driver pulls a large
  dependency tree (TLS, DNS resolver, BSON). Applications that never touch
  MongoDB must not pay that compile-time or binary cost (NFR-Sca-02), mirroring
  the existing `view`/`ai`/`redis` gating precedent (ADR-0008).

Constraints: the workspace glob `members = ["crates/*", "xtask"]` auto-includes
new crates, so no root manifest edit is needed; every public item is documented;
files stay ≤500 lines; the default build and test run must not require a live
MongoDB server.

## Decision

**Ship MongoDB support as a new, self-contained `rustasea-mongo` crate behind an
optional `mongo` feature on the `rustasea` umbrella, with its own document
model, CRUD helpers, and typed errors — never routed through the SQL ORM.**

- New crate `crates/rustasea-mongo` depends on `mongodb` (bson 3 via the
  driver's `bson-3` feature), `bson`, `serde`, `thiserror`, `tokio`, and `toml`.
- Public surface: `MongoConfig` (`from_env` / `from_toml`), `MongoClient`
  (`connect`, `ping`, typed `collection::<T>()`), the `Document` trait
  (`const COLLECTION`, `fn id`), a generic `Collection<T>` with
  `insert_one`/`insert_many`/`find`/`find_one`/`update_one`/`delete_one`/`count`,
  and pragmatic `Filter`/`Update` builders that serialize to `bson::Document`.
- `MongoError` has four variants — `Connection`, `Configuration`,
  `Serialization`, `Operation` — and `map_driver_error` classifies the driver's
  `#[non_exhaustive]` `ErrorKind` into them so application matches stay stable.
- `crates/rustasea`: optional path dependency plus a `mongo` feature; `lib.rs`
  re-exports the module and curated types under `#[cfg(feature = "mongo")]`.
- `config/mongo.toml` documents `uri`, `database`, and an optional `[mongo.pool]`
  table; `MONGODB_URI`/`MONGODB_DATABASE` override at runtime.
- Unit tests require no server; the one live CRUD test is
  `#[ignore = "requires MONGODB_URI"]`.

## Alternatives

| Option | Pros | Cons | Verdict |
|--------|------|------|---------|
| **Feature-gated `rustasea-mongo` crate (chosen)** | Clean separation; default build free of the driver tree; document-native API; reuses the ADR-0008 gating precedent | Consumers enable `features = ["mongo"]`; two persistence APIs to learn | **Chosen** |
| Force documents into the SQL ORM (`rustasea-orm`) | One persistence API; no new crate | Documents are schemaless and non-relational — a column/table `Model` cannot represent nested BSON without leaky abstractions; couples a large driver tree into every ORM build; violates pay-for-what-you-use | Rejected — semantic mismatch and cost |
| Skip MongoDB entirely | Zero new code, zero new deps | Ignores explicit user demand for a popular document store; framework stays SQL-only | Rejected — fails the stated requirement |
| Runtime-pluggable driver behind a unified `Repository` trait | Single API across SQL + Mongo | A lowest-common-denominator trait cannot expose documents, aggregation, or BSON without erasing the very features MongoDB users want; large speculative surface | Rejected — premature abstraction (deferred to a future ADR if demand appears) |

## Consequences

- Default `cargo build -p rustasea` pulls no MongoDB driver; `cargo build -p
  rustasea --features mongo` links `mongodb`/`bson` and the `mongo` module.
- `cargo test -p rustasea-mongo` runs entirely without a server (bson serde
  round-trip, filter/update serialization, config parsing/validation, invalid
  URI → typed error); live verification is opt-in via `MONGODB_URI`.
- Positive: document apps get a native, documented API; the SQL path is
  untouched; the driver cost is opt-in (NFR-Sca-02).
- Negative: two persistence paradigms now coexist. Mitigated by keeping the
  crate deliberately non-ORM (no schema registry, no migration engine) and by
  documenting that it is a thin layer over the official driver.
- Neutral: unified SQL+Mongo integration (shared config, cross-store
  transactions) is explicitly out of scope and may become a follow-up ADR.
- The `Document` trait and `Filter`/`Update` builders are intentionally minimal;
  extending them is additive and non-breaking.

---

> **Note:** This ADR establishes `rustasea-mongo` as the canonical home for
> MongoDB support. Any future unification with the SQL ORM must supersede this
> ADR rather than expand `rustasea-orm`.
