# ADR-0006 — Workspace Crate Boundaries per FR Domain

> **Status:** Accepted
> **Date:** 2026-09-07
> **Deciders:** Tech Lead, Architecture
> **Milestone:** M0 (applies M0–M6)
> **Related:** BR-08/C-02/C-05 · FR-000–FR-612 · FS-M0-01–FS-M6-07 · architecture.md §3 · domain.md §2 · component-inventory.md

## Context

RustaSea must satisfy incremental adoption (BR-08, NFR-Sca-02): `cargo check -p rustasea-router` must not pull ORM, queue, or AI dependencies. A single `laravel/framework`-style monolithic crate would violate pay-for-crates-you-use and couple compilation across milestones. Conversely, over-fragmentation (one crate per FR) would inflate publish and CI cost. The bounded contexts from `domain.md` (BC-0–BC-6) and milestones M0–M6 suggest a natural crate grouping, but the boundary must be documented and enforced as a DAG with no cycles.

Constraints: workspace `resolver = "2"` with `[workspace.dependencies]` as single source of truth; each crate `cargo check`-clean standalone (C-05); proc-macros isolated to `rustasea-macros`; re-export via umbrella `rustasea`.

## Decision

**One crate per milestone domain, plus shared foundation and umbrella — with a strict DAG and umbrella re-export.**

Inventory clarification (GAP-022, verified 2026-09-17 via `cargo metadata --format-version 1 --no-deps`): the current workspace has **42 crates under `crates/` + `xtask` (43 workspace packages total)**. The original decision named one crate per milestone domain; the ADOPT adoption wave (ADOPT-001..031) and the starter-kit programme (ADR-0002) subsequently added the audit, i18n, timezone, openapi, debugbar, queue-dashboard, excel, image, google, modules, action, logging, and presentation/scaffolder crates. Each addition keeps the same one-crate-per-concern boundary and joins the existing DAG without a cycle. The [canonical crate inventory](../../.agents/documents/application/modules/manifest.md#canonical-crate-inventory-source-of-truth) enumerates all 43 members by milestone and adoption layer, and supersedes the original count and optional crate splits; the boundary decision itself remains unchanged.

Structure (see `architecture.md §3`):

- `rustasea` (umbrella, re-exports only, no logic; also ships the `cargo-rustasea` scaffolder binary behind the `scaffold` feature, installed via `cargo install rustasea`)
- M0: `rustasea-foundation` (includes `Container`; no separate container crate), `rustasea-config`, `rustasea-macros` (proc-macro)
- M1: `rustasea-router`, `rustasea-http`
- M2: `rustasea-orm`
- M3: `rustasea-auth`, `rustasea-validation`
- M4: `rustasea-queue`, `rustasea-cache`, `rustasea-events`, `rustasea-schedule`
- M5: `rustasea-cli`, `rustasea-testing`
- M6: `rustasea-broadcast`, `rustasea-storage`, `rustasea-search`, `rustasea-ai`, `rustasea-jsonapi` (separate crate)
- `rustasea-app` (runnable example; `publish = false`)
- the `cargo-rustasea` scaffolder binary (`cargo rustasea new`) now ships in the `rustasea` umbrella package, not as a standalone crate
- `xtask` (workspace dev tooling, outside `crates/`)

The original per-domain plan above is extended by the ADOPT adoption wave and the starter-kit programme (ADR-0002), each crate still mapping to a single concern and a single milestone:

- M0/M1/M2 additions: `rustasea-openapi` (ADOPT-011, M1), `rustasea-activitylog` (ADOPT-002, M2), `rustasea-mongo` (ADR-0010, feature-gated)
- M3 additions: `rustasea-i18n` (ADOPT-005), `rustasea-authlog` (ADOPT-003), `rustasea-timezone` (ADOPT-006)
- M4 additions: `rustasea-queue-dashboard` (ADOPT-021), `rustasea-debugbar` (ADOPT-009)
- M5 additions: `rustasea-logging` (ADOPT-004/014), `rustasea-action` (ADOPT-028), `rustasea-modules` (ADOPT-027), `rustasea-scaffold` (ADR-0002)
- M6 additions: `rustasea-mail`, `rustasea-excel` (ADOPT-023), `rustasea-image` (ADOPT-024), `rustasea-google` (ADOPT-026), and the presentation family `rustasea-view`, `rustasea-inertia`, `rustasea-inertia-client`, `rustasea-inertia-adapters`, `rustasea-livewire` (ADR-0002)

All additions remain feature-gated where they would otherwise couple compilation across milestones, preserving the pay-for-crates-you-use rule (BR-08, NFR-Sca-02).

Edges are compile-time `depends on`; acyclicity validated by `xtask check-cycles` over `cargo metadata`. Cross-context communication is via domain events (`Dispatcher`) or shared kernel types from `rustasea-foundation`.

## Alternatives

| Option | Pros | Cons | Verdict |
|--------|------|------|---------|
| **Per-domain crates + umbrella (chosen)** | Matches BC boundaries; incremental adoption via feature flags; clear ownership per milestone; `cargo tree` audit proves isolation | Multiple crates increase the publish/CI matrix | **Chosen** — balances modularity with maintainability; single workspace manifest keeps versioning coherent |
| Monolithic `rustasea` crate | Simplest publish, single version | Violates BR-08; every consumer pulls AI/pgvector/redis; compile times scale with full feature surface | Rejected — directly contradicts NFR-Sca-02 |
| One crate per FR (≈70 crates) | Maximal pay-per-feature granularity | Explosive publish overhead; dependency diamond risk; proc-macro per FR is wasteful | Rejected — fragmentation cost exceeds benefit |
| Feature-flagged single crate with modules | No crate sprawl; still feature-gated | Isolation is not enforced by Cargo — a consumer can accidentally depend on non-enabled module types; DAG not visible | Rejected — crate boundary is stronger than feature boundary |

## Consequences

- `rustasea` umbrella re-exports domain crates with matching feature flags; consumer selects `rustasea = { features = ["router","orm"] }` or individual crates.
- `cargo check -p rustasea-router` has no `sqlx`/`async-openai` in `cargo tree` — enforced in CI.
- Each domain crate owns its typed errors (`ContainerError`, `QueryError`, …) and traits; new cross-crate decisions require an ADR and an update to `architecture.md §3`.
- Negative: publishing requires coordinating versions across publishable crates (the example app has `publish = false`); mitigated by `cargo xtask release --dry-run` and `release-plz`.
- Neutral: `xtask` is not a framework crate but ships in the same repo for `check-cycles` and generation tooling.

---

> **Archive note (rebrand 2026-09-09):** project renamed from Rustavel to **RustaSea**.
> This document is archived as-is under the historical `Rustavel` name for traceability;
> current branding is RustaSea (`rustasea` crates, `RustaSea` prose).
