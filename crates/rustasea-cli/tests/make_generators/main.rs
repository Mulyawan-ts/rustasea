//! Generator e2e smoke tests — every `make:*` kind writes a real, non-empty
//! `.rs` file whose source names the requested scaffold (FS-M5-02, FR-501).
//!
//! Scaffolds land in a unique tempdir per case, so the repository tree is
//! never touched. Compile/rustfmt cleanliness of the emitted sources is
//! exercised separately against an app fixture (see the M5 pipeline) — these
//! tests lock the generator *wiring*: correct output path, non-trivial source,
//! `AlreadyExists` guard and `--force` overwrite.
//!
//! The cases are grouped by generator family into submodules; the shared
//! tempdir helper lives in [`common`].

mod class;
mod common;
mod http;
mod model_migration;
mod test_kinds;
