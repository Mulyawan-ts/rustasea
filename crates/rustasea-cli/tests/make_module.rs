//! `make:module` e2e tests (ADOPT-027).
//!
//! The generator writes a whole workspace crate rather than a single class, so
//! these tests assert the file set, the framework wiring, and — through `syn`
//! as an independent oracle — that the emitted Rust actually parses.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rustasea_cli::generators::{generate, Kind, MakeOptions};

/// Unique temp root per invocation, removed on success.
fn temp_root() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "rustasea-cli-module-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    root
}

/// Options for a PascalCase module scaffold.
fn options(name: &str, force: bool) -> MakeOptions {
    MakeOptions {
        name: name.to_string(),
        force,
        resource: false,
        with_migration: false,
    }
}

/// `make:module Blog` writes the full module crate layout.
#[test]
fn make_module_writes_complete_crate() {
    let root = temp_root();
    let files = generate(Kind::Module, &root, &options("Blog", false)).expect("module scaffold");

    let written: Vec<String> = files.iter().map(|file| file.path.clone()).collect();
    let expected = [
        "modules/blog/Cargo.toml",
        "modules/blog/src/lib.rs",
        "modules/blog/src/routes.rs",
        "modules/blog/src/providers/mod.rs",
        "modules/blog/config/blog.toml",
        "modules/blog/migrations/.gitkeep",
    ];
    assert_eq!(written, expected);
    for relative in expected {
        assert!(root.join(relative).is_file(), "{relative} was not written");
    }

    let lib = std::fs::read_to_string(root.join("modules/blog/src/lib.rs")).expect("lib source");
    assert!(lib.contains("use rustasea::modules::Module;"), "{lib}");
    assert!(lib.contains("impl Module for Blog"), "{lib}");
    assert!(lib.contains("pub mod routes;"), "{lib}");

    let routes =
        std::fs::read_to_string(root.join("modules/blog/src/routes.rs")).expect("routes source");
    assert!(
        routes.contains("pub fn register(router: &mut Router)"),
        "{routes}"
    );
    assert!(routes.contains(r#"router.get("/blog");"#), "{routes}");

    let manifest =
        std::fs::read_to_string(root.join("modules/blog/Cargo.toml")).expect("cargo manifest");
    assert!(manifest.contains(r#"name = "module-blog""#), "{manifest}");
    assert!(manifest.contains(r#"features = ["modules"]"#), "{manifest}");

    let config =
        std::fs::read_to_string(root.join("modules/blog/config/blog.toml")).expect("config");
    assert!(config.contains("[module]"), "{config}");

    let _ = std::fs::remove_dir_all(&root);
}

/// The generated Rust is syntactically valid (parsed by `syn`, not a substring).
#[test]
fn module_sources_parse_as_rust() {
    let root = temp_root();
    generate(Kind::Module, &root, &options("Blog", false)).expect("module scaffold");

    for relative in ["modules/blog/src/lib.rs", "modules/blog/src/routes.rs"] {
        let source = std::fs::read_to_string(root.join(relative)).expect("read source");
        syn::parse_file(&source).unwrap_or_else(|error| {
            panic!("{relative} is not valid Rust: {error}");
        });
    }

    let _ = std::fs::remove_dir_all(&root);
}

/// A framework source checkout gets a path dependency; a generated app does not.
#[test]
fn module_manifest_matches_project_layout() {
    let root = temp_root();
    generate(Kind::Module, &root, &options("Blog", false)).expect("module scaffold");
    let manifest =
        std::fs::read_to_string(root.join("modules/blog/Cargo.toml")).expect("cargo manifest");
    assert!(manifest.contains(r#"version = "0.1""#), "{manifest}");

    // Simulate the framework source checkout by exposing `crates/rustasea`.
    std::fs::create_dir_all(root.join("crates/rustasea")).expect("framework dir");
    std::fs::write(root.join("crates/rustasea/Cargo.toml"), "[package]\n").expect("framework");
    generate(Kind::Module, &root, &options("Shop", false)).expect("module scaffold");
    let manifest =
        std::fs::read_to_string(root.join("modules/shop/Cargo.toml")).expect("cargo manifest");
    assert!(
        manifest.contains(r#"path = "../../crates/rustasea""#),
        "{manifest}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A duplicate module name is rejected unless `--force` overwrites it.
#[test]
fn duplicate_module_is_rejected() {
    let root = temp_root();
    generate(Kind::Module, &root, &options("Blog", false)).expect("first scaffold");

    let again = generate(Kind::Module, &root, &options("Blog", false));
    assert!(
        again.is_err(),
        "second scaffold without --force must report AlreadyExists"
    );

    let forced = generate(Kind::Module, &root, &options("Blog", true));
    assert!(forced.is_ok(), "--force must overwrite the module crate");

    let _ = std::fs::remove_dir_all(&root);
}

/// A non-PascalCase module name is rejected before anything is written.
#[test]
fn module_rejects_invalid_name() {
    let root = temp_root();
    let error = generate(Kind::Module, &root, &options("blog", false)).expect_err("invalid name");
    assert!(
        matches!(error, rustasea_cli::CliError::GenerationFailed { .. }),
        "expected a typed GenerationFailed, got {error:?}"
    );
    assert!(!root.join("modules/blog").exists());

    let _ = std::fs::remove_dir_all(&root);
}
