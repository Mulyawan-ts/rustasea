# Contributing to RustaSea

Thanks for your interest in RustaSea, a Rust framework with Laravel ergonomics.
This guide covers prerequisites, local setup, the checks that run in CI, and the
conventions the project follows. By participating you agree to keep the
repository's `master` branch green.

## Prerequisites

- **Rust 1.88 or newer.** The workspace minimum supported Rust version (MSRV) is
  1.88, set by `ADR-0001` (jsonwebtoken 10 and workspace MSRV 1.88) and declared
  in `Cargo.toml` (`[workspace.package] rust-version = "1.88"`). The CI `msrv`
  job pins 1.88.0 so an accidental use of a newer API fails the build.
- **A `cargo` on `PATH`.** RustaSea does not vendor a toolchain. Install Rust via
  [rustup](https://rustup.rs) and confirm `cargo --version` reports 1.88 or
  newer. The repository's `rust-toolchain.toml` selects the `stable` channel with
  the `rustfmt` and `clippy` components; rustup installs them automatically.
- **Git** for the normal clone/branch/PR flow.
- **Docker** is optional. It is only needed for the container dev stack and for
  the Docker-backed integration test suite (`--features integration`).

## Getting started

```bash
# Clone and enter the workspace.
git clone git@github.com:rustasea/rustasea.git
cd rustasea

# Build the whole workspace.
cargo build

# Run the workspace test suite.
cargo test --workspace
```

The workspace is a Cargo virtual manifest whose members are the crates under
`crates/*` plus the `xtask` helper crate (`Cargo.toml`). `cargo build` at the
root builds every member.

## Development checks

Run the same gates CI runs before you open a pull request.

```bash
# Full local CI gate: fmt, then clippy (-D warnings), then deps:check,
# then lines:check, then the crate-DAG cycle check.
cargo xtask ci

# Individual tasks.
cargo xtask fmt          # cargo fmt --all -- --check
cargo xtask clippy       # cargo clippy --workspace --all-targets -- -D warnings
cargo xtask deps:check   # fail on dependency-version drift (see STD-002)
cargo xtask lines:check  # fail on any source file over the 500-line limit (ADR-0009)
cargo xtask check-cycles # validate the crate dependency graph stays acyclic
cargo xtask migrate      # run the framework's registered migrations

# Supply-chain gate: licenses, advisories, bans, and source provenance.
cargo deny check

# RustSec advisory scan (CI runs this as a required job).
cargo audit
```

`cargo xtask` is a convenience alias defined in `.cargo/config.toml`; it expands
to `cargo run -p xtask --`. The task surface is implemented in
`xtask/src/main.rs` (`ci`, `fmt`, `clippy`, `check-cycles`, `deps:check`,
`lines:check`, `migrate`, and the `docker:up` / `docker:down` / `docker:logs`
helpers).

To format the tree in place rather than check it:

```bash
cargo fmt --all
```

### Docker development (optional)

The workspace ships a compose stack for a local app plus Postgres, Redis, MinIO,
and Mailpit. See the README "Docker development" section for the full flow.

```bash
cargo xtask docker:up     # up -d, then prints the service URLs
cargo xtask docker:logs   # follow the logs
cargo xtask docker:down   # stop and remove
```

### Docker-backed integration tests (optional)

The integration suite requires a running Docker daemon and is opt-in:

```bash
cargo test -p rustasea --features integration -- --ignored
```

## Branch and commit conventions

- **Branch from `master`.** Use a short, descriptive branch name such as
  `feat/queue-dashboard` or `fix/route-binding`.
- **Conventional Commits.** Commit subjects follow the Conventional Commits form
  with an optional task scope in square brackets, for example:

  ```text
  feat(scope): [GAP-022] description
  ```

  The type is one of `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `ci`,
  or `style`. The scope is a short area name (for example `auth`, `orm`, `cli`,
  `router`, `docs`). The `[TASK-ID]` token is optional but strongly encouraged;
  when present it names the task the change closes (`GAP-*`, `ADOPT-*`,
  `LARAVEL-*`, `AUTH-*`, `CFG-*`, and so on). This mirrors the existing history,
  for example `feat(orm): [ADOPT-020] JSON-column relations with dialect overlap
  predicates and batched eager loading`.
- **Keep commits focused.** One logical change per commit; do not mix a
  refactor with a behaviour change. Keep generated code `rustfmt`-clean.
- **Do not commit secrets.** Use `.env` locally (see `.env.example`); never
  commit credentials, tokens, or private keys.

## Pull request process

1. Open the PR against `master` and fill in a clear description: what changed,
   why, and how you verified it.
2. The `auto-assign-reviewer` workflow requests a review from the maintainer on
   every non-draft PR that targets `master`.
3. CI runs the following jobs (`.github/workflows/ci.yml`) and all must pass:

   | Job | What it runs |
   |---|---|
   | `quality` | `cargo xtask ci` (fmt, clippy with `-D warnings`, `deps:check`, `lines:check`, cycle check) |
   | `test` | `cargo test --workspace` |
   | `deny` | `cargo deny check` |
   | `audit` | `cargo audit` |
   | `msrv` | `cargo check --workspace` on Rust 1.88.0 |

   Formatting violations fail the build: run `cargo fmt --all` before pushing.
4. Address review feedback with new commits; avoid rewriting shared history.

## Coding standards

- **500-line file limit.** Every source file is capped at 500 lines (excluding
  generated trees). A file that must exceed the limit needs a documented
  exception recorded in an ADR; see `ADR-0009` for the one existing exemption
  (`.agents/documents/requirements/fsd.md`). Split a file that grows past the
  limit rather than leaving it oversized. `cargo xtask lines:check` enforces the
  limit for `.rs` sources and runs as part of `cargo xtask ci`.
- **No `unwrap()` or `expect()` in non-test code.** Framework crates are
  expected to use typed errors (for example `thiserror`-derived enums) instead
  of panicking on fallible calls; `rustasea-auth` carries
  `#![deny(clippy::unwrap_used)]` and `#![deny(clippy::expect_used)]` with a
  `cfg(test)` allowance as the reference pattern. Return or propagate a typed
  error rather than panicking.
- **English documentation.** Doc comments (`///` for items, `//!` for modules)
  are written in English and explain intent, not just restate the signature.
- **Workspace dependency inheritance (`STD-002`).** Every dependency shared by
  two or more workspace members is declared once in the root
  `[workspace.dependencies]` table and inherited with `workspace = true`. Do not
  pin an inline version for a crate already present there; express extra
  features additively (`dep = { workspace = true, features = ["extra"] }`).
  `cargo xtask deps:check` fails CI on drift and runs as part of
  `cargo xtask ci`.
- **No raw commit SHAs in documentation (`STD-001`).** Documentation cites
  milestone IDs, task IDs, and `path:line` references rather than volatile
  commit hashes. See `docs/documentation-conventions.md`.
- **Formatting.** `rustfmt` is configured in `rustfmt.toml` (edition 2021,
  `max_width = 100`); clippy thresholds live in `clippy.toml`. Generated code
  must be `rustfmt`-clean.

## Where to find the design and architecture docs

- `README.md` - project overview, CLI reference, tech stack, and roadmap.
- `docs/milestones.md` - the authoritative, evidence-backed per-milestone
  status (`M0`-`M6`).
- `docs/adr/` - Architecture Decision Records; start at `docs/adr/README.md`
  for the index.
- `docs/laravel-parity.md` - the Laravel 13.x API adoption mapping.
- `.agents/documents/` - requirements, design, and application documentation
  (blueprint, architecture, API contracts, module manifests).
- `docs/gap-analysis/` - the gap analysis and traceability matrix.

## License

By contributing, you agree that your contributions are licensed under the MIT
License (`LICENSE-MIT`), the same license that covers the project.
