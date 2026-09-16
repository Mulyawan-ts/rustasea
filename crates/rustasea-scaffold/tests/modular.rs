//! `cargo rustasea new --modular` layout tests (ADOPT-027).
//!
//! The modular flag turns the generated application into a workspace whose
//! `modules/*` members are the crates produced by `make:module`. These tests
//! pin both the modular output and the unchanged default output.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rustasea_scaffold::{Scaffold, StarterKitVariant};

/// Unique temp root per invocation, removed on success.
fn temp_root() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "rustasea-scaffold-modular-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    root
}

/// Render a scaffold and index the files by path.
fn render(modular: bool) -> BTreeMap<String, String> {
    let files = Scaffold::new("my-app", StarterKitVariant::Blade)
        .with_modular(modular)
        .render()
        .expect("render succeeds");
    files
        .into_iter()
        .map(|file| (file.path, file.contents))
        .collect()
}

/// The modular manifest enables the `modules` feature and declares the
/// `modules/*` workspace members.
#[test]
fn modular_manifest_wires_the_workspace() {
    for variant in StarterKitVariant::ALL {
        let files = Scaffold::new("my-app", variant)
            .with_modular(true)
            .render()
            .expect("render succeeds");
        let cargo = files
            .iter()
            .find(|file| file.path == "Cargo.toml")
            .expect("Cargo.toml rendered");
        assert!(
            cargo.contents.contains("[workspace]"),
            "{variant:?} lacks a workspace stanza: {}",
            cargo.contents
        );
        assert!(
            cargo.contents.contains(r#"members = ["modules/*"]"#),
            "{variant:?} lacks the modules member glob"
        );
        assert!(
            cargo.contents.contains(r#"features = ["modules", "#),
            "{variant:?} lacks the modules umbrella feature"
        );
        assert!(
            files.iter().any(|file| file.path == "modules/.gitkeep"),
            "{variant:?} lacks the modules marker"
        );
    }
}

/// Without `--modular` the manifest is unchanged and no `modules/` is emitted.
#[test]
fn default_manifest_stays_non_modular() {
    let files = render(false);
    let cargo = files.get("Cargo.toml").expect("Cargo.toml rendered");
    assert!(!cargo.contains("[workspace]"), "{cargo}");
    assert!(!cargo.contains(r#""modules""#), "{cargo}");
    assert!(!files.contains_key("modules/.gitkeep"));
}

/// The modular layout is written to disk by `generate`, marker included.
#[test]
fn modular_layout_writes_modules_marker() {
    let root = temp_root();
    let generated = Scaffold::new("my-app", StarterKitVariant::Blade)
        .with_modular(true)
        .generate(&root)
        .expect("generate succeeds");

    assert!(
        generated.iter().any(|file| file.path == "modules/.gitkeep"),
        "generated file list must include the modules marker"
    );
    assert!(root.join("modules/.gitkeep").is_file());

    let cargo = std::fs::read_to_string(root.join("Cargo.toml")).expect("read Cargo.toml");
    assert!(cargo.contains(r#"members = ["modules/*"]"#), "{cargo}");

    let _ = std::fs::remove_dir_all(&root);
}
