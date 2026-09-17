//! Retired route-definition copy — superseded by the crate-local app routes.
//!
//! This file is **not compiled**: no crate in the workspace includes the
//! workspace-root `routes/` directory. There is no `include!`, `#[path]`, or
//! `mod` referring to it, the root has no `Cargo.toml` package (the workspace
//! members are `crates/*` and `xtask`), and no `build.rs` emits it. The only
//! references to a `routes/web.rs` path elsewhere in the tree are
//! `crates/rustasea/tests/cli.rs:78` and
//! `crates/rustasea-scaffold/tests/scaffold.rs:41`, which assert that the
//! *scaffold generator* writes that path into a generated application — not
//! this workspace-root file.
//!
//! Historically this was a divergent second implementation of `/`, `/health`,
//! and `/welcome` (minijinja-rendered, `service: "rustasea"`) that shadowed
//! nothing and served nothing. The canonical, compiled route definitions now
//! live in `crates/rustasea-app/src/routes/` (`mod.rs`, `web.rs`, `auth.rs`,
//! `settings.rs`, `console.rs`), which the `rustasea-app` binary compiles and
//! serves via `try_into_axum_router()`.
//!
//! Kept as a documented shim so the layout path is not silently reintroduced
//! with a second, divergent implementation.
