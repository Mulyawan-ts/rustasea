//! Developer-tooling templates - formatting config and the CI workflow.
//!
//! Every generated application ships a `rustfmt.toml` mirroring the framework
//! repository's own formatting contract and a minimal GitHub Actions workflow
//! (`.github/workflows/ci.yml`) that runs `cargo fmt --check`, `cargo clippy
//! --all-targets -- -D warnings`, and `cargo test` on stable. The workflow is
//! deliberately generic: it never references workspace-only tooling (`cargo
//! xtask`, `cargo deny`, `cargo audit`) that a generated app does not have.

use super::TemplateFile;

/// Tooling templates (shared by every variant).
pub fn entries() -> Vec<TemplateFile> {
    vec![
        ("rustfmt.toml", RUSTFMT_TOML),
        (".github/workflows/ci.yml", CI_WORKFLOW),
    ]
}

// Mirrors the framework repository's `rustfmt.toml` so a generated application
// formats identically to the framework it depends on.
const RUSTFMT_TOML: &str = r##"# Rustfmt configuration.
#
# These are intentionally the rustfmt defaults, pinned explicitly so the
# formatting contract is version-controlled and stable across toolchains.
# `max_width = 100` matches the framework design system.

edition = "2021"
max_width = 100
tab_spaces = 4
"##;

// Minimal generated-app quality gate: format, lint, and test on stable. Kept
// free of workspace-only tooling (`cargo xtask`, `cargo deny`, `cargo audit`)
// that a generated application does not carry. Both `main` and `master` are
// watched because `git init` picks either depending on the user's config.
const CI_WORKFLOW: &str = r##"name: CI

# Quality gate for @@app_pascal@@: formatting, lint, and tests on stable.
on:
  push:
    branches: [main, master]
  pull_request:
    branches: [main, master]

# Least privilege: the workflow only reads repository contents.
permissions:
  contents: read

# Cancel superseded runs for the same ref so re-pushes do not stack jobs.
concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always

jobs:
  quality:
    name: Format, lint & test
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install stable toolchain (rustfmt + clippy)
        uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - name: cargo fmt
        run: cargo fmt --all -- --check
      - name: cargo clippy
        run: cargo clippy --all-targets -- -D warnings
      - name: cargo test
        run: cargo test
"##;
