//! Manifest integration tests: resolution semantics, format-preserving saves,
//! and on-disk module discovery.

use std::path::{Path, PathBuf};

use rustasea_modules::{
    discover_modules, ModuleManifest, APP_MANIFEST, MODULES_DIR, MODULES_MANIFEST,
};

/// Write `contents` to `<dir>/modules.toml` and load the manifest.
fn manifest_with(dir: &Path, contents: &str) -> ModuleManifest {
    let path = dir.join("modules.toml");
    std::fs::write(&path, contents).expect("write manifest");
    ModuleManifest::load(&path).expect("load manifest")
}

/// A module is enabled by default; `disabled` always wins.
#[test]
fn disabled_always_wins() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest = manifest_with(
        dir.path(),
        "[modules]\nenabled = [\"blog\"]\ndisabled = [\"blog\"]\n",
    );
    assert!(!manifest.is_enabled("blog"));
    assert!(!manifest.is_enabled("shop"), "allow-list excludes the rest");
}

/// An absent `[modules]` table enables everything.
#[test]
fn empty_manifest_enables_everything() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest = manifest_with(dir.path(), "[app]\nname = \"demo\"\n");
    assert!(!manifest.is_allow_list());
    assert!(manifest.is_enabled("blog"));
}

/// Saving preserves unrelated tables/comments and survives a round-trip.
#[test]
fn save_preserves_unrelated_content_and_reloads() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("rustasea.toml");
    std::fs::write(&path, "# app manifest\n[app]\nname = \"demo\"\n").expect("seed manifest");

    let mut manifest = ModuleManifest::load(&path).expect("load manifest");
    manifest.disable("admin");
    manifest.enable("blog");
    manifest.save().expect("save manifest");

    let text = std::fs::read_to_string(&path).expect("read manifest");
    assert!(text.contains("# app manifest"), "comment lost: {text}");
    assert!(text.contains("name = \"demo\""), "table lost: {text}");

    let reloaded = ModuleManifest::load(&path).expect("reload manifest");
    assert!(reloaded.is_enabled("blog"));
    assert!(!reloaded.is_enabled("admin"));
    assert!(reloaded.is_enabled("shop"), "default-on mode preserved");
    assert_eq!(reloaded.path(), path);
}

/// An active allow-list round-trips and prunes empty deny-lists.
#[test]
fn save_round_trips_and_prunes_empty_keys() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("modules.toml");
    std::fs::write(
        &path,
        "[modules]\nenabled = [\"admin\"]\ndisabled = [\"blog\"]\n",
    )
    .expect("seed manifest");

    let mut manifest = ModuleManifest::load(&path).expect("load manifest");
    assert!(manifest.is_allow_list());
    manifest.enable("blog");
    manifest.save().expect("save manifest");

    let text = std::fs::read_to_string(&path).expect("read manifest");
    assert!(text.contains("enabled = [\"admin\", \"blog\"]"), "{text}");
    assert!(!text.contains("disabled"), "empty deny-list kept: {text}");

    let reloaded = ModuleManifest::load(&path).expect("reload manifest");
    assert!(reloaded.is_enabled("blog"));
    assert!(reloaded.is_enabled("admin"));
    assert!(!reloaded.is_enabled("shop"), "allow-list still active");

    manifest.disable("admin");
    manifest.save().expect("save manifest");
    let text = std::fs::read_to_string(&path).expect("read manifest");
    assert!(text.contains("disabled = [\"admin\"]"), "{text}");
    assert!(text.contains("enabled = [\"blog\"]"), "{text}");

    let reloaded = ModuleManifest::load(&path).expect("reload manifest");
    assert!(!reloaded.is_enabled("admin"));
    assert!(reloaded.is_enabled("blog"));
    assert!(!reloaded.is_enabled("shop"), "allow-list survives pruning");
}

/// Enabling in default-on mode never narrows the other modules.
#[test]
fn enable_in_default_on_mode_keeps_others_enabled() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("modules.toml");
    std::fs::write(&path, "[modules]\ndisabled = [\"admin\"]\n").expect("seed manifest");

    let mut manifest = ModuleManifest::load(&path).expect("load manifest");
    manifest.enable("blog");
    manifest.save().expect("save manifest");

    let reloaded = ModuleManifest::load(&path).expect("reload manifest");
    assert!(!reloaded.is_allow_list());
    assert!(reloaded.is_enabled("blog"));
    assert!(reloaded.is_enabled("shop"), "sibling must stay enabled");
    assert!(!reloaded.is_enabled("admin"));
}

/// The dedicated manifest wins over `rustasea.toml`.
#[test]
fn read_prefers_dedicated_then_app_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("rustasea.toml"),
        "[modules]\ndisabled = [\"blog\"]\n",
    )
    .expect("seed app manifest");
    let fallback = ModuleManifest::read(dir.path()).expect("read fallback");
    assert!(!fallback.is_enabled("blog"));
    assert_eq!(fallback.path(), dir.path().join(APP_MANIFEST));

    std::fs::create_dir_all(dir.path().join("config")).expect("mkdir config");
    std::fs::write(
        dir.path().join(MODULES_MANIFEST),
        "[modules]\nenabled = [\"blog\"]\n",
    )
    .expect("seed dedicated manifest");
    let dedicated = ModuleManifest::read(dir.path()).expect("read dedicated");
    assert!(dedicated.is_enabled("blog"));
    assert_eq!(dedicated.path(), dir.path().join(MODULES_MANIFEST));
}

/// Discovery reads versions, sorts names, and skips non-crates.
#[test]
fn discover_reads_version_and_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let modules = dir.path().join(MODULES_DIR);
    for name in ["shop", "blog"] {
        let crate_dir = modules.join(name);
        std::fs::create_dir_all(crate_dir.join("config")).expect("mkdir module");
        std::fs::write(crate_dir.join("Cargo.toml"), "[package]\nname = \"x\"\n")
            .expect("seed Cargo.toml");
        std::fs::write(
            crate_dir.join("config").join(format!("{name}.toml")),
            "[module]\nversion = \"1.2.3\"\n",
        )
        .expect("seed config");
    }
    std::fs::create_dir_all(modules.join("not-a-module")).expect("mkdir stray");

    let manifest = manifest_with(dir.path(), "[modules]\ndisabled = [\"shop\"]\n");
    let found = discover_modules(dir.path(), &manifest).expect("discover");
    let names: Vec<&str> = found.iter().map(|module| module.name.as_str()).collect();
    assert_eq!(names, vec!["blog", "shop"], "sorted, stray ignored");
    assert_eq!(found[0].path, PathBuf::from("modules/blog"));
    assert_eq!(found[0].version.as_deref(), Some("1.2.3"));
    assert!(found[0].enabled);
    assert!(!found[1].enabled);
}

/// A missing `modules/` directory is not an error.
#[test]
fn discover_missing_dir_is_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest = manifest_with(dir.path(), "");
    assert!(discover_modules(dir.path(), &manifest)
        .expect("discover")
        .is_empty());
}
